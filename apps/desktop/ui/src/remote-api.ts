import { invoke } from "@tauri-apps/api/core";
import type {
  AgentRuntimeStatus,
  AgentSettings,
  ServerCheckResult,
  ServerConfig,
  ServerConfigState,
} from "./types";

export type LegacyControllerConfig = {
  controlServerUrl: string;
  relayAddress: string;
  relayServerName: string;
  caCertificatePath: string;
};

export const remoteXApi = {
  loadServerConfig(legacyController: LegacyControllerConfig): Promise<ServerConfigState> {
    return invoke("load_server_config", { legacyController });
  },
  saveServerConfig(config: ServerConfig): Promise<ServerConfig> {
    return invoke("save_server_config", { config });
  },
  checkServerConnection(config: ServerConfig): Promise<ServerCheckResult> {
    return invoke("check_server_connection", { config });
  },
  resetFirstRun(): Promise<ServerConfigState> {
    return invoke("reset_first_run");
  },
  loadAgentSettings(): Promise<AgentSettings> {
    return invoke("load_agent_settings");
  },
  saveAgentSettings(settings: AgentSettings): Promise<void> {
    return invoke("save_agent_settings", { settings });
  },
  agentStatus(): Promise<AgentRuntimeStatus> {
    return invoke("agent_status");
  },
  startAgent(): Promise<AgentRuntimeStatus> {
    return invoke("start_agent");
  },
  stopAgent(): Promise<AgentRuntimeStatus> {
    return invoke("stop_agent");
  },
};

