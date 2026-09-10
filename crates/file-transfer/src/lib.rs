//! Safe rooted browsing and bounded, resumable, SHA-256 verified file transfers.

use remotex_protocol::{
    DEFAULT_FILE_CHUNK_SIZE, FileEntry, FileEntryKind, MAX_DIRECTORY_ENTRIES, MAX_FILE_CHUNK_SIZE,
    MAX_FILE_PATH_SIZE, TransferId,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};
use thiserror::Error;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
};

/// Maximum open upload/download handles of one direction in one Session.
pub const MAX_CONCURRENT_TRANSFERS_PER_SESSION: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllowedRoot {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
struct CanonicalRoot {
    name: String,
    path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RootedFileSystem {
    roots: Vec<CanonicalRoot>,
}

impl RootedFileSystem {
    /// Creates a virtual filesystem whose first component selects an allowed root.
    pub fn new(roots: Vec<AllowedRoot>) -> Result<Self, FileTransferError> {
        if roots.is_empty() {
            return Err(FileTransferError::NoAllowedRoots);
        }
        let mut canonical = Vec::with_capacity(roots.len());
        for root in roots {
            validate_root_name(&root.name)?;
            if !root.path.is_absolute() {
                return Err(FileTransferError::RootIsNotAbsolute(root.path));
            }
            if canonical
                .iter()
                .any(|candidate: &CanonicalRoot| candidate.name.eq_ignore_ascii_case(&root.name))
            {
                return Err(FileTransferError::DuplicateRoot(root.name));
            }
            let path = std::fs::canonicalize(&root.path).map_err(FileTransferError::Io)?;
            if !path.is_dir() {
                return Err(FileTransferError::RootIsNotDirectory(path));
            }
            canonical.push(CanonicalRoot {
                name: root.name,
                path,
            });
        }
        Ok(Self { roots: canonical })
    }

    #[must_use]
    pub fn virtual_roots(&self) -> Vec<FileEntry> {
        self.roots
            .iter()
            .map(|root| FileEntry {
                name: root.name.clone(),
                path: format!("/{}", root.name),
                kind: FileEntryKind::Directory,
                size: 0,
                modified_ms: None,
            })
            .collect()
    }

    pub async fn list_directory(
        &self,
        virtual_path: &str,
    ) -> Result<Vec<FileEntry>, FileTransferError> {
        if virtual_path == "/" {
            return Ok(self.virtual_roots());
        }
        let resolved = self.resolve_existing(virtual_path).await?;
        let metadata = fs::metadata(&resolved.physical).await.map_err(map_io)?;
        if !metadata.is_dir() {
            return Err(FileTransferError::NotDirectory);
        }
        let mut reader = fs::read_dir(&resolved.physical).await.map_err(map_io)?;
        let mut entries = Vec::new();
        while let Some(entry) = reader.next_entry().await.map_err(map_io)? {
            if entries.len() >= MAX_DIRECTORY_ENTRIES {
                return Err(FileTransferError::TooManyDirectoryEntries);
            }
            let metadata = entry.metadata().await.map_err(map_io)?;
            let kind = if metadata.is_dir() {
                FileEntryKind::Directory
            } else if metadata.is_file() {
                FileEntryKind::File
            } else {
                continue;
            };
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| FileTransferError::NonUnicodeName)?;
            let path = join_virtual(virtual_path, &name)?;
            entries.push(FileEntry {
                name,
                path,
                kind,
                size: if kind == FileEntryKind::File {
                    metadata.len()
                } else {
                    0
                },
                modified_ms: modified_ms(&metadata),
            });
        }
        entries.sort_by(|left, right| {
            let left_rank = usize::from(left.kind == FileEntryKind::File);
            let right_rank = usize::from(right.kind == FileEntryKind::File);
            left_rank
                .cmp(&right_rank)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(entries)
    }

    pub async fn create_directory(&self, virtual_path: &str) -> Result<(), FileTransferError> {
        let target = self.resolve_new(virtual_path).await?;
        fs::create_dir(&target).await.map_err(map_io)
    }

    pub async fn open_download(
        &self,
        virtual_path: &str,
    ) -> Result<OutgoingTransfer, FileTransferError> {
        let resolved = self.resolve_existing(virtual_path).await?;
        OutgoingTransfer::open(resolved.physical, DEFAULT_FILE_CHUNK_SIZE).await
    }

    pub async fn open_upload(
        &self,
        transfer_id: TransferId,
        virtual_path: &str,
        total_size: u64,
        chunk_size: u32,
        sha256: [u8; 32],
    ) -> Result<IncomingTransfer, FileTransferError> {
        let final_path = self.resolve_new(virtual_path).await?;
        IncomingTransfer::open(transfer_id, final_path, total_size, chunk_size, sha256).await
    }

    async fn resolve_existing(
        &self,
        virtual_path: &str,
    ) -> Result<ResolvedPath, FileTransferError> {
        let (root, relative) = self.parse_virtual(virtual_path)?;
        let candidate = root.path.join(relative);
        let physical = fs::canonicalize(&candidate).await.map_err(map_io)?;
        if !physical.starts_with(&root.path) {
            return Err(FileTransferError::PathEscapesRoot);
        }
        Ok(ResolvedPath { physical })
    }

    async fn resolve_new(&self, virtual_path: &str) -> Result<PathBuf, FileTransferError> {
        let (root, relative) = self.parse_virtual(virtual_path)?;
        let name = relative
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or(FileTransferError::RootMutationDenied)?;
        let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
        let parent = fs::canonicalize(root.path.join(parent_relative))
            .await
            .map_err(map_io)?;
        if !parent.starts_with(&root.path) {
            return Err(FileTransferError::PathEscapesRoot);
        }
        let target = parent.join(name);
        match fs::symlink_metadata(&target).await {
            Ok(_) => Err(FileTransferError::AlreadyExists),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(target),
            Err(error) => Err(FileTransferError::Io(error)),
        }
    }

    fn parse_virtual<'a>(
        &'a self,
        virtual_path: &str,
    ) -> Result<(&'a CanonicalRoot, PathBuf), FileTransferError> {
        validate_virtual_path(virtual_path)?;
        let mut parts = virtual_path.trim_start_matches('/').split('/');
        let root_name = parts.next().ok_or(FileTransferError::InvalidPath)?;
        let root = self
            .roots
            .iter()
            .find(|root| root.name == root_name)
            .ok_or(FileTransferError::UnknownRoot)?;
        let mut relative = PathBuf::new();
        for part in parts {
            if part.is_empty() || part == "." || part == ".." {
                return Err(FileTransferError::InvalidPath);
            }
            relative.push(part);
        }
        Ok((root, relative))
    }
}

#[derive(Debug)]
struct ResolvedPath {
    physical: PathBuf,
}

#[derive(Debug)]
pub struct IncomingTransfer {
    transfer_id: TransferId,
    final_path: PathBuf,
    partial_path: PathBuf,
    file: File,
    total_size: u64,
    chunk_size: u32,
    expected_sha256: [u8; 32],
    next_offset: u64,
}

impl IncomingTransfer {
    pub async fn open(
        transfer_id: TransferId,
        final_path: PathBuf,
        total_size: u64,
        chunk_size: u32,
        expected_sha256: [u8; 32],
    ) -> Result<Self, FileTransferError> {
        validate_chunk_size(chunk_size)?;
        match fs::symlink_metadata(&final_path).await {
            Ok(_) => return Err(FileTransferError::AlreadyExists),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(FileTransferError::Io(error)),
        }
        let partial_path = partial_path(&final_path, transfer_id)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&partial_path)
            .await
            .map_err(map_io)?;
        let mut next_offset = file.metadata().await.map_err(map_io)?.len();
        if next_offset > total_size {
            file.set_len(0).await.map_err(map_io)?;
            next_offset = 0;
        }
        file.seek(SeekFrom::Start(next_offset))
            .await
            .map_err(map_io)?;
        Ok(Self {
            transfer_id,
            final_path,
            partial_path,
            file,
            total_size,
            chunk_size,
            expected_sha256,
            next_offset,
        })
    }

    #[must_use]
    pub const fn transfer_id(&self) -> TransferId {
        self.transfer_id
    }

    #[must_use]
    pub const fn next_offset(&self) -> u64 {
        self.next_offset
    }

    #[must_use]
    pub const fn total_size(&self) -> u64 {
        self.total_size
    }

    pub async fn write_chunk(
        &mut self,
        offset: u64,
        checksum: [u8; 32],
        payload: &[u8],
    ) -> Result<u64, FileTransferError> {
        if offset != self.next_offset {
            return Err(FileTransferError::UnexpectedOffset {
                expected: self.next_offset,
                actual: offset,
            });
        }
        if payload.is_empty() || payload.len() > self.chunk_size as usize {
            return Err(FileTransferError::InvalidChunk);
        }
        let next = offset
            .checked_add(u64::try_from(payload.len()).map_err(|_| FileTransferError::InvalidChunk)?)
            .ok_or(FileTransferError::ExceedsDeclaredSize)?;
        if next > self.total_size || hash_bytes(payload) != checksum {
            return Err(if next > self.total_size {
                FileTransferError::ExceedsDeclaredSize
            } else {
                FileTransferError::ChunkChecksumMismatch
            });
        }
        self.file.write_all(payload).await.map_err(map_io)?;
        self.file.flush().await.map_err(map_io)?;
        self.next_offset = next;
        Ok(next)
    }

    pub async fn complete(
        mut self,
        total_size: u64,
        sha256: [u8; 32],
    ) -> Result<(), FileTransferError> {
        if total_size != self.total_size || self.next_offset != self.total_size {
            return Err(FileTransferError::Incomplete {
                expected: self.total_size,
                actual: self.next_offset,
            });
        }
        if sha256 != self.expected_sha256 {
            return Err(FileTransferError::ChecksumMismatch);
        }
        self.file.flush().await.map_err(map_io)?;
        drop(self.file);
        if hash_file(&self.partial_path).await? != self.expected_sha256 {
            return Err(FileTransferError::ChecksumMismatch);
        }
        fs::rename(&self.partial_path, &self.final_path)
            .await
            .map_err(map_io)
    }

    pub async fn cancel(self) -> Result<(), FileTransferError> {
        drop(self.file);
        match fs::remove_file(self.partial_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(FileTransferError::Io(error)),
        }
    }
}

#[derive(Debug)]
pub struct OutgoingTransfer {
    path: PathBuf,
    file: File,
    total_size: u64,
    chunk_size: u32,
    sha256: [u8; 32],
}

impl OutgoingTransfer {
    pub async fn open(path: PathBuf, chunk_size: u32) -> Result<Self, FileTransferError> {
        validate_chunk_size(chunk_size)?;
        let metadata = fs::metadata(&path).await.map_err(map_io)?;
        if !metadata.is_file() {
            return Err(FileTransferError::NotFile);
        }
        let total_size = metadata.len();
        let sha256 = hash_file(&path).await?;
        let file = File::open(&path).await.map_err(map_io)?;
        Ok(Self {
            path,
            file,
            total_size,
            chunk_size,
            sha256,
        })
    }

    #[must_use]
    pub const fn total_size(&self) -> u64 {
        self.total_size
    }

    #[must_use]
    pub const fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    #[must_use]
    pub const fn sha256(&self) -> [u8; 32] {
        self.sha256
    }

    pub fn filename(&self) -> Result<String, FileTransferError> {
        self.path
            .file_name()
            .and_then(OsStr::to_str)
            .map(ToOwned::to_owned)
            .ok_or(FileTransferError::NonUnicodeName)
    }

    pub async fn read_chunk(
        &mut self,
        offset: u64,
    ) -> Result<Option<(u64, [u8; 32], Vec<u8>)>, FileTransferError> {
        if offset > self.total_size {
            return Err(FileTransferError::UnexpectedOffset {
                expected: self.total_size,
                actual: offset,
            });
        }
        if offset == self.total_size {
            return Ok(None);
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .await
            .map_err(map_io)?;
        let remaining = self.total_size - offset;
        let length = usize::try_from(remaining.min(u64::from(self.chunk_size)))
            .map_err(|_| FileTransferError::InvalidChunk)?;
        let mut payload = vec![0_u8; length];
        self.file.read_exact(&mut payload).await.map_err(map_io)?;
        let checksum = hash_bytes(&payload);
        Ok(Some((offset, checksum, payload)))
    }
}

pub async fn hash_file(path: &Path) -> Result<[u8; 32], FileTransferError> {
    let mut file = File::open(path).await.map_err(map_io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(map_io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn validate_root_name(name: &str) -> Result<(), FileTransferError> {
    if name.is_empty()
        || name.len() > 64
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(FileTransferError::InvalidRootName);
    }
    Ok(())
}

fn validate_virtual_path(path: &str) -> Result<(), FileTransferError> {
    if path.is_empty()
        || path.len() > MAX_FILE_PATH_SIZE
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.contains('\\')
        || Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::CurDir | Component::Prefix(_)
            )
        })
    {
        return Err(FileTransferError::InvalidPath);
    }
    Ok(())
}

fn validate_chunk_size(chunk_size: u32) -> Result<(), FileTransferError> {
    if chunk_size == 0 || chunk_size > MAX_FILE_CHUNK_SIZE {
        return Err(FileTransferError::InvalidChunkSize);
    }
    Ok(())
}

fn join_virtual(parent: &str, name: &str) -> Result<String, FileTransferError> {
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(FileTransferError::InvalidPath);
    }
    Ok(if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    })
}

fn partial_path(final_path: &Path, transfer_id: TransferId) -> Result<PathBuf, FileTransferError> {
    let filename = final_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or(FileTransferError::NonUnicodeName)?;
    Ok(final_path.with_file_name(format!(".{filename}.remotex-{transfer_id}.part")))
}

fn modified_ms(metadata: &std::fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

fn map_io(error: std::io::Error) -> FileTransferError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FileTransferError::NotFound,
        std::io::ErrorKind::AlreadyExists => FileTransferError::AlreadyExists,
        std::io::ErrorKind::PermissionDenied => FileTransferError::PermissionDenied,
        _ => FileTransferError::Io(error),
    }
}

#[derive(Debug, Error)]
pub enum FileTransferError {
    #[error("at least one allowed root is required")]
    NoAllowedRoots,
    #[error("allowed root name is invalid")]
    InvalidRootName,
    #[error("allowed root name is duplicated: {0}")]
    DuplicateRoot(String),
    #[error("allowed root is not a directory: {0}")]
    RootIsNotDirectory(PathBuf),
    #[error("allowed root is not absolute: {0}")]
    RootIsNotAbsolute(PathBuf),
    #[error("virtual path is invalid")]
    InvalidPath,
    #[error("virtual path selects an unknown root")]
    UnknownRoot,
    #[error("path escapes its allowed root")]
    PathEscapesRoot,
    #[error("allowed roots cannot be modified directly")]
    RootMutationDenied,
    #[error("path was not found")]
    NotFound,
    #[error("path already exists")]
    AlreadyExists,
    #[error("filesystem permission denied")]
    PermissionDenied,
    #[error("path is not a directory")]
    NotDirectory,
    #[error("path is not a regular file")]
    NotFile,
    #[error("filesystem name is not valid Unicode")]
    NonUnicodeName,
    #[error("directory has too many entries")]
    TooManyDirectoryEntries,
    #[error("too many concurrent file transfers")]
    TooManyTransfers,
    #[error("chunk size must be between 1 and 4 MiB")]
    InvalidChunkSize,
    #[error("file chunk is empty or exceeds the negotiated limit")]
    InvalidChunk,
    #[error("expected offset {expected}, received {actual}")]
    UnexpectedOffset { expected: u64, actual: u64 },
    #[error("chunk exceeds the declared file size")]
    ExceedsDeclaredSize,
    #[error("file chunk checksum does not match")]
    ChunkChecksumMismatch,
    #[error("completed file checksum does not match")]
    ChecksumMismatch,
    #[error("transfer is incomplete: expected {expected} bytes, received {actual}")]
    Incomplete { expected: u64, actual: u64 },
    #[error("filesystem operation failed: {0}")]
    Io(#[source] std::io::Error),
}

#[derive(Debug)]
pub struct TransferRegistry<T> {
    entries: HashMap<TransferId, T>,
}

impl<T> Default for TransferRegistry<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<T> TransferRegistry<T> {
    pub fn insert(&mut self, id: TransferId, transfer: T) -> Result<(), FileTransferError> {
        if self.entries.contains_key(&id) {
            return Err(FileTransferError::AlreadyExists);
        }
        if self.entries.len() >= MAX_CONCURRENT_TRANSFERS_PER_SESSION {
            return Err(FileTransferError::TooManyTransfers);
        }
        self.entries.insert(id, transfer);
        Ok(())
    }

    pub fn get_mut(&mut self, id: &TransferId) -> Result<&mut T, FileTransferError> {
        self.entries.get_mut(id).ok_or(FileTransferError::NotFound)
    }

    pub fn remove(&mut self, id: &TransferId) -> Result<T, FileTransferError> {
        self.entries.remove(id).ok_or(FileTransferError::NotFound)
    }

    #[must_use]
    pub fn contains(&self, id: &TransferId) -> bool {
        self.entries.contains_key(id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("remotex-{label}-{}", TransferId::new()));
        fs::create_dir(&path).expect("create test directory");
        path
    }

    #[test]
    fn transfer_registry_has_a_hard_session_limit() {
        let mut registry = TransferRegistry::default();
        for value in 0..MAX_CONCURRENT_TRANSFERS_PER_SESSION {
            registry
                .insert(TransferId::new(), value)
                .expect("insert bounded transfer");
        }
        assert!(matches!(
            registry.insert(TransferId::new(), usize::MAX),
            Err(FileTransferError::TooManyTransfers)
        ));
    }

    fn rooted(path: &Path) -> RootedFileSystem {
        RootedFileSystem::new(vec![AllowedRoot {
            name: "Data".into(),
            path: path.into(),
        }])
        .expect("create rooted filesystem")
    }

    #[tokio::test]
    async fn lists_roots_and_directory_metadata() {
        let directory = temp_directory("list");
        fs::write(directory.join("hello.txt"), b"hello").expect("write fixture");
        fs::create_dir(directory.join("folder")).expect("create fixture directory");
        let filesystem = rooted(&directory);
        assert_eq!(
            filesystem
                .list_directory("/")
                .await
                .expect("list roots")
                .len(),
            1
        );
        let entries = filesystem
            .list_directory("/Data")
            .await
            .expect("list directory");
        assert_eq!(entries[0].kind, FileEntryKind::Directory);
        assert_eq!(entries[1].size, 5);
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn rejects_traversal_and_unknown_roots() {
        let directory = temp_directory("paths");
        let filesystem = rooted(&directory);
        assert!(matches!(
            filesystem.list_directory("/Data/../secret").await,
            Err(FileTransferError::InvalidPath)
        ));
        assert!(matches!(
            filesystem.list_directory("/Unknown").await,
            Err(FileTransferError::UnknownRoot)
        ));
        assert!(matches!(
            filesystem.create_directory("/Data/../../escape").await,
            Err(FileTransferError::InvalidPath)
        ));
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn rejects_relative_allowed_roots() {
        assert!(matches!(
            RootedFileSystem::new(vec![AllowedRoot {
                name: "Data".into(),
                path: PathBuf::from("relative"),
            }]),
            Err(FileTransferError::RootIsNotAbsolute(_))
        ));
    }

    #[tokio::test]
    async fn creates_one_directory_without_recursive_delete_support() {
        let directory = temp_directory("mkdir");
        let filesystem = rooted(&directory);
        filesystem
            .create_directory("/Data/new")
            .await
            .expect("create directory");
        assert!(directory.join("new").is_dir());
        assert!(matches!(
            filesystem.create_directory("/Data/new").await,
            Err(FileTransferError::AlreadyExists)
        ));
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn small_upload_is_chunked_and_verified() {
        let directory = temp_directory("upload");
        let filesystem = rooted(&directory);
        let bytes = b"RemoteX upload";
        let hash = hash_bytes(bytes);
        let id = TransferId::new();
        let mut incoming = filesystem
            .open_upload(id, "/Data/result.bin", bytes.len() as u64, 5, hash)
            .await
            .expect("start upload");
        for (index, chunk) in bytes.chunks(5).enumerate() {
            let offset = (index * 5) as u64;
            incoming
                .write_chunk(offset, hash_bytes(chunk), chunk)
                .await
                .expect("write chunk");
        }
        incoming
            .complete(bytes.len() as u64, hash)
            .await
            .expect("complete upload");
        assert_eq!(
            fs::read(directory.join("result.bin")).expect("read uploaded file"),
            bytes
        );
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn interrupted_upload_resumes_from_partial_length() {
        let directory = temp_directory("resume");
        let filesystem = rooted(&directory);
        let bytes = b"0123456789";
        let id = TransferId::new();
        let hash = hash_bytes(bytes);
        let mut first = filesystem
            .open_upload(id, "/Data/resume.bin", 10, 4, hash)
            .await
            .expect("start upload");
        first
            .write_chunk(0, hash_bytes(&bytes[..4]), &bytes[..4])
            .await
            .expect("write first chunk");
        drop(first);
        let resumed = filesystem
            .open_upload(id, "/Data/resume.bin", 10, 4, hash)
            .await
            .expect("resume upload");
        assert_eq!(resumed.next_offset(), 4);
        resumed.cancel().await.expect("cancel partial");
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn rejects_invalid_offset_and_checksum() {
        let directory = temp_directory("invalid");
        let filesystem = rooted(&directory);
        let id = TransferId::new();
        let mut incoming = filesystem
            .open_upload(id, "/Data/bad.bin", 4, 4, hash_bytes(b"good"))
            .await
            .expect("start upload");
        assert!(matches!(
            incoming.write_chunk(1, hash_bytes(b"bad!"), b"bad!").await,
            Err(FileTransferError::UnexpectedOffset { .. })
        ));
        assert!(matches!(
            incoming.write_chunk(0, [0; 32], b"bad!").await,
            Err(FileTransferError::ChunkChecksumMismatch)
        ));
        incoming.cancel().await.expect("cancel partial");
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn download_streams_multiple_chunks_and_supports_offsets() {
        let directory = temp_directory("download");
        let bytes = vec![7_u8; 1025];
        fs::write(directory.join("large.bin"), &bytes).expect("write fixture");
        let mut outgoing = OutgoingTransfer::open(directory.join("large.bin"), 256)
            .await
            .expect("open download");
        let mut offset = 0;
        let mut output = Vec::new();
        while let Some((actual, checksum, payload)) =
            outgoing.read_chunk(offset).await.expect("read chunk")
        {
            assert_eq!(actual, offset);
            assert_eq!(hash_bytes(&payload), checksum);
            offset += payload.len() as u64;
            output.extend_from_slice(&payload);
        }
        assert_eq!(output, bytes);
        assert!(matches!(
            outgoing.read_chunk(1026).await,
            Err(FileTransferError::UnexpectedOffset { .. })
        ));
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn large_file_uses_bounded_default_chunks() {
        let directory = temp_directory("large-default");
        let default_chunk_size =
            usize::try_from(DEFAULT_FILE_CHUNK_SIZE).expect("chunk size fits usize");
        let bytes = vec![11_u8; default_chunk_size + 17];
        fs::write(directory.join("large.bin"), &bytes).expect("write fixture");
        let mut outgoing =
            OutgoingTransfer::open(directory.join("large.bin"), DEFAULT_FILE_CHUNK_SIZE)
                .await
                .expect("open large download");
        let (_, _, first) = outgoing
            .read_chunk(0)
            .await
            .expect("read first chunk")
            .expect("first chunk exists");
        let (_, _, second) = outgoing
            .read_chunk(u64::from(DEFAULT_FILE_CHUNK_SIZE))
            .await
            .expect("read second chunk")
            .expect("second chunk exists");
        assert_eq!(first.len(), default_chunk_size);
        assert_eq!(second.len(), 17);
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn checksum_mismatch_preserves_partial_for_retry() {
        let directory = temp_directory("checksum");
        let filesystem = rooted(&directory);
        let bytes = b"content";
        let id = TransferId::new();
        let mut incoming = filesystem
            .open_upload(id, "/Data/content.bin", bytes.len() as u64, 16, [9; 32])
            .await
            .expect("start upload");
        incoming
            .write_chunk(0, hash_bytes(bytes), bytes)
            .await
            .expect("write content");
        assert!(matches!(
            incoming.complete(bytes.len() as u64, [9; 32]).await,
            Err(FileTransferError::ChecksumMismatch)
        ));
        assert!(!directory.join("content.bin").exists());
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn cancellation_removes_partial_file() {
        let directory = temp_directory("cancel");
        let filesystem = rooted(&directory);
        let bytes = b"cancel me";
        let id = TransferId::new();
        let mut incoming = filesystem
            .open_upload(
                id,
                "/Data/cancel.bin",
                bytes.len() as u64,
                4,
                hash_bytes(bytes),
            )
            .await
            .expect("start upload");
        incoming
            .write_chunk(0, hash_bytes(&bytes[..4]), &bytes[..4])
            .await
            .expect("write partial chunk");
        incoming.cancel().await.expect("cancel transfer");
        assert_eq!(fs::read_dir(&directory).expect("read directory").count(), 0);
        fs::remove_dir_all(directory).expect("remove fixture");
    }
}
