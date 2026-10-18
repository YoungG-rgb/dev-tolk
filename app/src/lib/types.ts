export type Status = 'QUEUED' | 'PROCESSING' | 'COMPLETED' | 'FAILED' | 'CANCELLED';

export type Job = {
  id: string;
  sourceName: string;
  status: Status;
  durationMs: number | null;
  progress: number;
  attempts: number;
  hasEdits: boolean;
  rating: number | null;
  errorCode: string | null;
  errorMessage: string | null;
  errorAction: string | null;
  createdAt: number;
  finishedAt: number | null;
};

export type AppEvent =
  | { kind: 'started'; jobId: string }
  | { kind: 'progress'; jobId: string; percent: number }
  | { kind: 'finished'; jobId: string; status: Status; errorCode: string | null; errorMessage: string | null }
  | { kind: 'importStarted'; index: number; total: number; name: string }
  | { kind: 'importProgress'; index: number; percent: number }
  | { kind: 'importAccepted'; index: number; name: string; jobId: string; warnings: string[] }
  | { kind: 'importRejected'; index: number; name: string; code: string; message: string; action: string }
  | { kind: 'importBatchDone'; total: number; accepted: number; rejected: number; cancelled: boolean }
  | { kind: 'importBusy' }
  | { kind: 'importNotice'; message: string }
  | { kind: 'downloadProgress'; title: string; fileIndex: number; fileCount: number; downloaded: number; total: number; bytesPerSec: number }
  | { kind: 'modelTaskDone'; ok: boolean; cancelled: boolean; message: string | null; action: string | null; suggestImport: boolean }
  | { kind: 'flushAndClose' };

export type TranscriptData = {
  title: string;
  model: string;
  durationMs: number;
  hasEdits: boolean;
  segments: { startMs: number; endMs: number; text: string }[];
};

export type ModelInfo = {
  id: string;
  title: string;
  sizeBytes: number;
  requiredBytes: number;
  partialBytes: number;
  installed: boolean;
  isDefault: boolean;
  recommended: boolean;
  ramEstimateMb: number | null;
  source: string;
  license: string;
  note: string | null;
  memoryWarning: string | null;
};

export type ModelsOverview = {
  models: ModelInfo[];
  firstRun: boolean;
  os: string;
  totalRamMb: number;
  availableRamMb: number;
  recommendation: string;
  lowMemoryWarning: boolean;
  busy: boolean;
};

export type DownloadProgress = { title: string; fileIndex: number; fileCount: number; downloaded: number; total: number; bytesPerSec: number };
export type TaskResult = { ok: boolean; cancelled: boolean; message: string | null; action: string | null; suggestImport: boolean };

export type Diagnostics = {
  version: string;
  os: string;
  arch: string;
  dataDir: string;
  logDir: string;
  jobsBytes: number;
  modelsBytes: number;
  metricsCount: number;
};
