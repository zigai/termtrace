import { Recording, Termtrace } from './index';

export interface WithTermtraceContext {
  termtrace: Termtrace;
  record: (...args: Parameters<Termtrace['record']>) => Promise<Recording>;
  evidenceRoot: string;
}

export interface WithTermtraceOptions {
  binary?: string;
}

export declare function withTermtrace<T>(
  fn: (ctx: WithTermtraceContext) => Promise<T>,
  options?: WithTermtraceOptions,
): Promise<T>;
