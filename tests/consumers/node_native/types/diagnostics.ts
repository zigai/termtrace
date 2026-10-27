import { Termtrace, type SaveOutcome, type VerificationReport, type RegistryEntry, type ApplicationAcknowledgement, type TimelinePage } from '@termtrace/core'

function nested(saved: SaveOutcome, verified: VerificationReport, registered: RegistryEntry, ack: ApplicationAcknowledgement, timeline: TimelinePage): bigint {
  const directory: string | Uint8Array = saved.directory
  const trusted: boolean = verified.signature_signer_trusted
  const timestamp: bigint = registered.registered_unix_ns
  const seq: bigint = ack.boundaries.durable.next_event_seq
  timeline.entries.forEach(entry => { if (entry.item.kind === 'semantic_state') { const id: bigint = entry.item.state_id; void id } })
  void directory; void trusted
  // @ts-expect-error nested byte counts are exact, never JS numbers
  const rounded: number = saved.manifest.files[0].byte_count
  // @ts-expect-error wide timestamps cannot be narrowed by report declarations
  const unsafe: number = timestamp
  void rounded; void unsafe
  return timestamp + seq
}
async function surfaces(client: Termtrace): Promise<void> {
  await client.evidence(new Uint8Array([47])).save(new Uint8Array([47]), ['png', 'deep-json'], { selector: 'monotonic_ns', monotonic_ns: 9007199254740993n })
  await client.evidence(new Uint8Array([47])).verify('signature', new Uint8Array([47]))
  await client.registry().restart('demo', { command: [new Uint8Array([47])], output_byte_budget: 18446744073709551615n })
  await client.tmux().record({ name: 'demo', ghostty_profile: 'off', new_server: true, retain_bytes: 18446744073709551615n })
  // @ts-expect-error profiles are a closed policy, not bundle-supplied trust
  await client.evidence(new Uint8Array([47])).bundle(new Uint8Array([47]), 'automatic')
  // @ts-expect-error verification levels cannot silently downgrade
  await client.evidence(new Uint8Array([47])).verify('sig')
  // @ts-expect-error wide restart budgets require bigint
  await client.registry().restart('demo', { output_byte_budget: 10 })
  // @ts-expect-error application attributes remain ordinary bounded JSON, not byte buffers
  await client.instrumentation(new Uint8Array([47])).event('event', { value: new Uint8Array([1]) })
  // @ts-expect-error tmux coverage retains the underlying policy union
  await client.tmux().record({ coverage: 'secret' })
}
void nested; void surfaces

function nullableWide(registered: RegistryEntry, gap: import('@termtrace/core').TmuxGapCatalogEntry): void {
  const empty: RegistryEntry['recorder_process_start_ticks'] = null
  const unresolved: typeof gap.gap.resolved_unix_ns = null
  const options: import('@termtrace/core').RestartConfiguration = { output_byte_budget: null, max_duration_ms: null }
  // @ts-expect-error nullable counters require both absence and null guards
  const ticks: bigint = registered.recorder_process_start_ticks + 1n
  // @ts-expect-error unresolved gap timestamps require a null guard
  const end: bigint = gap.gap.resolved_unix_ns + 1n
  void empty; void unresolved; void options; void ticks; void end
}
