import {
  ActionResult, Termtrace, type LaunchOptions, type MutationFailure, type PointerOutcome,
  type SendStepSpec, type SessionStatus,
} from '@termtrace/core';

async function consumer(tt: Termtrace): Promise<void> {
  const options: LaunchOptions = {
    environment: { base: 'clear', set: { X: new Uint8Array([1, 255]) }, copy: ['HOME'] },
    output_byte_budget: 1024n, max_duration_ms: 100n,
  };
  const recording = await tt.launch(['/bin/sh'], options);
  const builder = recording.actions().withRequestId('019fddda-17c8-7000-87da-8785e71ab5e0').sendBytes(new Uint8Array([0]));
  const id: string = builder.requestId;
  const result: ActionResult = await builder.untilScreenWhere({ predicate: 'contains', text: 'OK' }, 500).run();
  const written: bigint = result.input.bytes_written;
  const hash: string | null | undefined = result.postcondition.state_hash;
  const pointer: PointerOutcome = await recording.pointer().drag(0, 0, 4, 0, { steps: 3, strict: true });
  if (pointer.kind === 'applied') {
    const raw: Uint8Array = pointer.bytes;
    void raw;
  }
  const steps: SendStepSpec[] = [
    { step: 'input', atoms: [{ bytes: new Uint8Array([27]) }, { key: 'enter' }] },
    { step: 'pointer', column: 1, row: 0, operation: { kind: 'scroll', lines: -1 } },
  ];
  const sent = await recording.send({ steps, request_id: id, until: { condition: 'idle', stable_for_ms: 50n } });
  const lastId: string = sent.last.request_id;
  const status: SessionStatus = await recording.status();
  if (status.kind === 'live') {
    const seq: bigint = status.boundaries.durable.next_event_seq;
    void seq;
  }
  try {
    await recording.mark('x');
  } catch (error) {
    const failure = error as MutationFailure;
    const failed: string = failure.request_id;
    const incomplete: ActionResult | undefined = failure.result;
    void failed; void incomplete;
  }
  const stopped = await recording.requestStop({ waitMs: 1000 });
  if (stopped.kind === 'completed') {
    const limit: bigint | undefined = stopped.completion.resource_limit?.limit;
    void limit;
  }
  void written; void hash; void lastId;

  // @ts-expect-error Byte fields are Uint8Array, never text.
  const textBytes: SendStepSpec = { step: 'input', atoms: [{ bytes: 'text' }] };
  // @ts-expect-error Output byte budgets are u64 and therefore bigint.
  const narrow: LaunchOptions = { output_byte_budget: 1024 };
  // @ts-expect-error Pointer buttons are a closed set.
  await recording.pointer().click(0, 0, { button: 'side' });
  // @ts-expect-error Wide result fields are bigint.
  const lossy: number = result.input.bytes_written;
  // @ts-expect-error Mutations take no AbortSignal.
  await builder.run({ signal: new AbortController().signal });
  void textBytes; void narrow; void lossy;
}
void consumer;
