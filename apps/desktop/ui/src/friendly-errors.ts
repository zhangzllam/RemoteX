export type FriendlyFailure = {
  category: "server" | "certificate" | "offline" | "approval" | "device" | "relay" | "unknown";
  title: string;
  message: string;
  action: string;
};

export function friendlyFailure(error: unknown): FriendlyFailure {
  const raw = String(error).replace(/^Error:\s*/i, "").trim();
  const lower = raw.toLocaleLowerCase();
  if (/certificate|tls|ca |unknown issuer|invalid peer/.test(lower)) {
    return { category: "certificate", title: "Certificate could not be verified", message: "RemoteX could not verify the server or Relay identity.", action: "Check the server address and CA certificate in Network settings." };
  }
  if (/timed out|timeout|could not reach|connect error|dns|network/.test(lower)) {
    return { category: "offline", title: "Server is not reachable", message: "The network or self-hosted server did not respond in time.", action: "Check your connection, then try again." };
  }
  if (/reject|denied|authorization|approval/.test(lower)) {
    return { category: "approval", title: "Connection was not approved", message: "The remote device declined the request or the approval expired.", action: "Ask someone at the remote device to approve the new request." };
  }
  if (/device.*offline|not found|device id/.test(lower)) {
    return { category: "device", title: "Device is unavailable", message: "RemoteX could not find an online device with that ID.", action: "Confirm the ID and make sure RemoteX is open on the other device." };
  }
  if (/relay|peer|session closed/.test(lower)) {
    return { category: "relay", title: "Secure path was interrupted", message: "The session could not complete its secure direct or Relay connection.", action: "Try again. If this continues, check the Relay address and UDP reachability." };
  }
  if (/server|http|url|address/.test(lower)) {
    return { category: "server", title: "Server configuration needs attention", message: raw || "The configured server rejected the request.", action: "Open Network settings and verify the server configuration." };
  }
  return { category: "unknown", title: "RemoteX could not complete the request", message: raw || "An unexpected error occurred.", action: "Try again. Open diagnostics if the problem continues." };
}

export function friendlyError(error: unknown): string {
  const failure = friendlyFailure(error);
  return `${failure.message} ${failure.action}`;
}

