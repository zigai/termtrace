import type {
  AnnotationPosition, AnnotationScope, AnnotationStyle, CaptionLayout, ChangeMode, ChangeSource,
  EditPlan, EditSelector, EditSource, ExactPath, NormalizedEdit, RationalSpeed,
} from './types'

/** An `Edit`, an `EditSource`, or the path of a JSON edit file. */
export type EditInput = Edit | EditSource | ExactPath

/** One source coordinate: exactly one marker, time, event, state or native frame. */
export declare class Selector {
  private constructor()
  readonly wire: Readonly<EditSelector>
  toWire(): EditSelector
}

/** Selector constructors. 64-bit values are `bigint` or a safe-integer `number`. */
export declare const select: {
  marker(label: string): Selector
  atNs(offset: bigint | number): Selector
  event(seq: bigint | number): Selector
  state(id: bigint | number): Selector
  /** Starts at `ceil(frame * 1e9 / fps)` ns, so it selects `frame` exactly. */
  nativeFrame(frame: bigint | number): Selector
}

/** Annotation lifetimes on the output timeline. */
export declare const scope: {
  wholeVideo(): AnnotationScope
  clip(index: bigint | number): AnnotationScope
  outputFrame(frame: bigint | number): AnnotationScope
  range(from: Selector, to: Selector): AnnotationScope
}

export interface ClipOptions {
  speed?: RationalSpeed
  hold_ms?: bigint | number
  caption?: string | null
}

export interface AnnotationOptions {
  text?: string | null
  template?: string | null
  scope: AnnotationScope
  position?: AnnotationPosition
  style?: AnnotationStyle
  z_order?: number
}

/**
 * Presentation edit builder. Methods append in call order and return the builder. Rounding:
 * clip start floors and end ceils to frames, retimed count is
 * `ceil(frames * denominator / numerator)`, hold is `ceil(hold_ms * fps / 1000)` frames, and
 * output frames map back to source frames with a floor.
 */
export declare class Edit {
  constructor(options?: { resource_base?: ExactPath | null })
  readonly resource_base: ExactPath | null
  clip(from: Selector, to: Selector, options?: ClipOptions): this
  annotate(annotation: AnnotationOptions): this
  subtitles(ass: ExactPath): this
  changes(source: ChangeSource, mode: ChangeMode, options?: { hold_frames?: number; pixel_threshold?: number | null }): this
  captionLayout(caption: CaptionLayout): this
  branding(label: string): this
  delivery(delivery: { codec: string; pixel_format: string; crf: number; encoder_preset: string }): this
  cursorMask(): this
  crop(x: number, y: number, width: number, height: number): this
  zoom(numerator: number, denominator: number): this
  resize(width: number, height: number): this
  callout(x: number, y: number, width: number, height: number, options?: { label?: string | null }): this
  /** A copy of the plan as built; sections never set keep the Rust defaults. */
  build(): EditPlan
  toSource(): EditSource
  /** Resolve, validate and hash the edit in Rust. */
  normalize(): Promise<NormalizedEdit>
}
