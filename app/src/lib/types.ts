export type Status = 'QUEUED' | 'PROCESSING' | 'COMPLETED' | 'FAILED' | 'CANCELLED';

export type Job = {
  id: string;
  sourceName: string;
  status: Status;
  durationMs: number | null;
  progress: number;
  attempts: number;
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
  | { kind: 'importBusy' };

export type TranscriptData = {
  title: string;
  model: string;
  durationMs: number;
  hasEdits: boolean;
  paragraphs: { startMs: number; text: string }[];
};
