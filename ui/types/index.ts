export type ServiceKind = "nginx" | "mysql" | "redis";

export type HealthStatus =
  | "stopped"
  | "running"
  | "warningPortNotListen"
  | "errorLogHasError"
  | "crashed";

export type InstanceStatus = "stopped" | "running" | "crashed";

export interface InstanceView {
  id: string;
  kind: ServiceKind;
  name: string;
  version: string;
  port: number;
  httpsPort: number;
  status: InstanceStatus;
  health: HealthStatus;
  pid?: number | null;
  dir: string;
  urls: string[];
  hasRootPassword: boolean;
}

export interface CreateInstanceOptions {
  kind: ServiceKind;
  name: string;
  version: string;
  port?: number | null;
  rootPassword?: string | null;
}

export interface CreateInstanceResult {
  instance: InstanceView;
  initialRootPassword?: string | null;
}

export interface VersionInfo {
  kind: ServiceKind;
  version: string;
  platform: string;
  installed: boolean;
  installedPath?: string | null;
  downloadUrl: string;
  mirrorUrl?: string | null;
  sha256: string;
  binPath: string;
  archiveType: string;
}

export interface DownloadProgress {
  kind: string;
  version: string;
  downloaded: number;
  total?: number | null;
  percent: number;
  stage: string;
}

export interface SiteConfig {
  serverName: string;
  root: string;
  index: string;
  customLocations: string;
  https: boolean;
  certDomain?: string | null;
}

export interface NginxGlobalConfig {
  workerProcesses: number;
  workerConnections: number;
  keepaliveTimeout: number;
  gzip: boolean;
  clientMaxBodySize: string;
  httpsPort: number;
}

export interface RedisConfigView {
  bind: string;
  port: number;
  hasPassword: boolean;
  appendonly: boolean;
  requiresRestart: boolean;
}

export interface RedisConfigInput {
  bind: string;
  port: number;
  requirepass: string | null;
  appendonly: boolean;
}

export interface MysqlUser {
  user: string;
  host: string;
  dbs: string[];
}

export interface CertificateInfo {
  domain: string;
  instanceId: string;
  instanceName: string;
  certPath: string;
  keyPath: string;
  issuedAt: string;
  expiresAt: string;
  daysLeft: number;
}

export type DownloadSource = "official" | "mirror";

export interface Settings {
  dataDir?: string | null;
  manifestUrl?: string | null;
  downloadSource: DownloadSource;
  stopInstancesOnClose: boolean;
  autostart: boolean;
  httpApiEnabled: boolean;
  httpApiPort: number;
  logLevel: string;
}

export interface SettingsResult {
  settings: Settings;
  restartRequired: boolean;
}

export interface DoctorCheck {
  name: string;
  ok: boolean;
  detail: string;
}

export interface DoctorReport {
  ok: boolean;
  checks: DoctorCheck[];
}

export interface LogChunk {
  path: string;
  content: string;
}

export interface PathsInfo {
  home: string;
  dbFile: string;
  versions: string;
  instances: string;
  cache: string;
  logs: string;
}
