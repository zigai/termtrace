/* Generated from schemas/driver-response.schema.json. DO NOT EDIT. */

/**
 * Cursor visual style.
 */
export type CursorStyle = "bar" | "block" | "underline" | "hollow_block";
/**
 * Exact predicate evaluated against the latest presented viewport.
 */
export type ScreenPredicate =
  | {
      predicate: "contains";
      /**
       * Exact UTF-8 substring.
       */
      text: string;
    }
  | {
      predicate: "absent";
      /**
       * Exact UTF-8 substring.
       */
      text: string;
    }
  | {
      /**
       * Rust-regex pattern, limited by the control service.
       */
      pattern: string;
      predicate: "matches_regex";
    }
  | {
      /**
       * Rust-regex pattern, limited by the control service.
       */
      pattern: string;
      predicate: "excludes_regex";
    }
  | {
      predicate: "row_contains";
      /**
       * Zero-based viewport row.
       */
      row: number;
      /**
       * Exact UTF-8 substring.
       */
      text: string;
    }
  | {
      predicate: "row_absent";
      /**
       * Zero-based viewport row.
       */
      row: number;
      /**
       * Exact UTF-8 substring.
       */
      text: string;
    }
  | {
      predicate: "region_contains";
      region: ScreenRegion;
      /**
       * Exact UTF-8 substring.
       */
      text: string;
    }
  | {
      predicate: "region_absent";
      region: ScreenRegion1;
      /**
       * Exact UTF-8 substring.
       */
      text: string;
    }
  | {
      /**
       * Optional zero-based viewport column.
       */
      column?: number | null;
      predicate: "cursor";
      /**
       * Optional zero-based viewport row.
       */
      row?: number | null;
      /**
       * Optional cursor shape requirement.
       */
      shape?: CursorStyle | null;
      /**
       * Optional cursor visibility requirement.
       */
      visible?: boolean | null;
    }
  | {
      /**
       * Required set/reset value.
       */
      enabled: boolean;
      /**
       * Stable mode selector.
       */
      mode:
        | {
            mode: "alternate_screen";
          }
        | {
            mode: "bracketed_paste";
          }
        | {
            mode: "wraparound";
          }
        | {
            mode: "synchronized_output";
          }
        | {
            mode: "ansi";
            /**
             * Mode number.
             */
            number: number;
          }
        | {
            mode: "dec";
            /**
             * Mode number.
             */
            number: number;
          };
      predicate: "mode";
    }
  | {
      /**
       * Equality relationship to require.
       */
      comparison: "unchanged" | "changed";
      /**
       * Lowercase BLAKE3 state hash.
       */
      hash: string;
      predicate: "state_hash";
    }
  | {
      predicate: "all";
      /**
       * Bounded non-empty predicate list.
       */
      predicates: ScreenPredicate[];
    }
  | {
      predicate: "any";
      /**
       * Bounded non-empty predicate list.
       */
      predicates: ScreenPredicate[];
    };
export type Base64Bytes = string;
/**
 * Stable detector selectable by `diagnose --check`.
 */
export type DiagnosticCheck = "full_redraw" | "transient" | "dirty" | "cursor" | "modes" | "palette" | "query_loop";
/**
 * One ordered session event.
 */
export type Event =
  | {
      /**
       * Exact bytes, including control sequences and invalid UTF-8.
       */
      data: string;
      type: "output";
    }
  | {
      /**
       * Exact emitted bytes.
       */
      data: string;
      /**
       * Structured semantic source of the exact emitted bytes.
       */
      descriptor:
        | {
            kind: "raw";
          }
        | {
            kind: "text";
          }
        | {
            /**
             * Alt modifier.
             */
            alt: boolean;
            /**
             * Control modifier.
             */
            control: boolean;
            /**
             * Pinned encoder identity.
             */
            encoder: string;
            /**
             * Stable symbolic key spelling.
             */
            key: string;
            kind: "key";
            /**
             * Shift modifier.
             */
            shift: boolean;
            /**
             * Super modifier.
             */
            super_key: boolean;
            /**
             * BLAKE3 of canonical JSON for the terminal mode snapshot.
             */
            terminal_modes_hash: string;
          }
        | {
            /**
             * Whether the actor applied bracketed-paste wrappers.
             */
            bracketed: boolean;
            kind: "paste";
            /**
             * BLAKE3 of canonical JSON for the terminal mode snapshot.
             */
            terminal_modes_hash: string;
          }
        | {
            /**
             * Pinned encoder identity.
             */
            encoder: string;
            /**
             * Whether the terminal gained (`true`) or lost (`false`) focus.
             */
            focused: boolean;
            kind: "focus";
            /**
             * BLAKE3 of canonical JSON for the terminal mode snapshot.
             */
            terminal_modes_hash: string;
          }
        | {
            /**
             * Button name for clicks, otherwise wheel direction.
             */
            button: string;
            /**
             * Complete click count, or zero for non-click operations.
             */
            click_count: number;
            /**
             * Terminal columns at encoding time.
             */
            cols: number;
            /**
             * Zero-based viewport column.
             */
            column: number;
            /**
             * Pinned encoder identity.
             */
            encoder: string;
            /**
             * Optional zero-based destination column for a drag.
             */
            end_column?: number | null;
            /**
             * Optional zero-based destination row for a drag.
             */
            end_row?: number | null;
            kind: "pointer";
            /**
             * Signed requested wheel lines, or zero for clicks.
             */
            lines: number;
            /**
             * `click`, `multi_click`, `drag`, or `scroll`.
             */
            operation: string;
            /**
             * Total terminal surface height in pixels.
             */
            pixel_height: number;
            /**
             * Total terminal surface width in pixels.
             */
            pixel_width: number;
            /**
             * Zero-based viewport row.
             */
            row: number;
            /**
             * Terminal rows at encoding time.
             */
            rows: number;
            /**
             * BLAKE3 of canonical JSON for the terminal mode snapshot.
             */
            terminal_modes_hash: string;
          }
        | {
            /**
             * Current grid columns.
             */
            cols: number;
            /**
             * Pinned encoder identity.
             */
            encoder: string;
            /**
             * Zero-based exclusive matched column.
             */
            end_column: number;
            kind: "activate";
            /**
             * Presented-state hash used for resolution and encoding.
             */
            presented_state_hash: string;
            /**
             * Zero-based matched row.
             */
            row: number;
            /**
             * Current grid rows.
             */
            rows: number;
            /**
             * Zero-based first matched column.
             */
            start_column: number;
            /**
             * Cell chosen for the click.
             */
            target_column: number;
            /**
             * BLAKE3 of canonical JSON for the terminal mode snapshot.
             */
            terminal_modes_hash: string;
            /**
             * Exact matched text.
             */
            text: string;
          };
      /**
       * High-level input category.
       */
      input_kind: "raw" | "text" | "keys" | "paste" | "focus" | "pointer";
      /**
       * Optional symbolic intent such as `ctrl-c` or `paste`.
       */
      intent?: string | null;
      /**
       * Identifier paired with the corresponding result.
       */
      operation_id: string;
      /**
       * Input origin.
       */
      source: "human" | "agent" | "host";
      type: "input_intent";
    }
  | {
      /**
       * Bytes written before success or failure.
       */
      bytes_written: number;
      /**
       * Stable diagnostic code on failure.
       */
      error_code?: string | null;
      /**
       * Identifier from the corresponding intent.
       */
      operation_id: string;
      /**
       * Whether the entire byte sequence was applied.
       */
      status: "applied" | "failed";
      type: "input_result";
    }
  | {
      /**
       * Columns after the resize.
       */
      cols: number;
      /**
       * Identifier paired with the corresponding result.
       */
      operation_id: string;
      /**
       * Total pixel height reported to the child.
       */
      pixel_height: number;
      /**
       * Total pixel width reported to the child.
       */
      pixel_width: number;
      /**
       * Rows after the resize.
       */
      rows: number;
      type: "resize_intent";
    }
  | {
      /**
       * Stable diagnostic code on failure.
       */
      error_code?: string | null;
      /**
       * Identifier from the corresponding intent.
       */
      operation_id: string;
      /**
       * Whether the new geometry was applied.
       */
      status: "applied" | "failed";
      type: "resize_result";
    }
  | {
      /**
       * Stable display name such as `SIGINT`.
       */
      name: string;
      /**
       * Platform signal number.
       */
      number: number;
      /**
       * Identifier paired with the corresponding result.
       */
      operation_id: string;
      type: "signal_intent";
    }
  | {
      /**
       * Stable diagnostic code on failure.
       */
      error_code?: string | null;
      /**
       * Identifier from the corresponding intent.
       */
      operation_id: string;
      /**
       * Whether the signal syscall succeeded.
       */
      status: "applied" | "failed";
      type: "signal_result";
    }
  | {
      /**
       * User-provided marker label.
       */
      label: string;
      /**
       * Caller-owned operation identifier used for durable deduplication.
       */
      operation_id: string;
      type: "marker";
    }
  | {
      /**
       * Query bytes emitted by the child.
       */
      query: string;
      /**
       * Component that generated the response.
       */
      responder: string;
      /**
       * Response bytes sent back to the child.
       */
      response: string;
      type: "query_response";
    }
  | {
      /**
       * Transition kind.
       */
      lifecycle_kind: "spawned" | "output_closed" | "stopping";
      /**
       * Child process ID when available.
       */
      pid?: number | null;
      type: "lifecycle";
    }
  | {
      /**
       * Exit status when the process exited normally.
       */
      code?: number | null;
      /**
       * Signal number when signal termination occurred.
       */
      signal?: number | null;
      /**
       * Platform signal description when available.
       */
      signal_name?: string | null;
      type: "exit";
    }
  | {
      /**
       * Stable diagnostic code.
       */
      code: string;
      /**
       * Redacted human-readable message.
       */
      message: string;
      type: "host_error";
    };

/**
 * Exactly one correlated driver response.
 */
export interface DriverResponse {
  /**
   * Identifier copied from the request.
   */
  request_id: string;
  /**
   * Typed success or stable error.
   */
  result:
    | {
        /**
         * Method-specific data.
         */
        data:
          | {
              /**
               * Live actor protocol version.
               */
              control_protocol_version: number;
              /**
               * Driver protocol version.
               */
              driver_protocol_version: number;
              /**
               * Canonical event format version.
               */
              event_log_version: number;
              kind: "hello";
              /**
               * Maximum newline-framed request bytes.
               */
              max_message_bytes: number;
              /**
               * Stable supported methods.
               */
              methods: string[];
              /**
               * Semantic state format version.
               */
              state_format_version: number;
            }
          | {
              /**
               * Canonical path.
               */
              canonical_session: string;
              /**
               * Whether completion evidence already exists.
               */
              completed: boolean;
              /**
               * Process-local handle.
               */
              handle: string;
              kind: "attached";
              /**
               * Immutable canonical session identity.
               */
              session_id: string;
            }
          | {
              /**
               * Exact actor result data.
               */
              data:
                | {
                    boundaries: ActorBoundaries;
                    kind: "acknowledged";
                  }
                | {
                    boundaries: ActorBoundaries1;
                    /**
                     * Number of bytes written completely.
                     */
                    bytes_written: number;
                    /**
                     * Exact bytes emitted by the actor.
                     */
                    data: string;
                    /**
                     * Semantic input category.
                     */
                    input_kind: "raw" | "text" | "keys" | "paste" | "focus" | "pointer";
                    /**
                     * Canonical intent event sequence.
                     */
                    intent_event_seq: number;
                    kind: "input_applied";
                    /**
                     * Canonical operation identifier (the caller request ID).
                     */
                    operation_id: string;
                    /**
                     * Canonical result event sequence.
                     */
                    result_event_seq: number;
                  }
                | {
                    boundaries: ActorBoundaries2;
                    /**
                     * Semantic input category.
                     */
                    input_kind: "raw" | "text" | "keys" | "paste" | "focus" | "pointer";
                    kind: "input_noop";
                    /**
                     * Stable explanation for the no-op.
                     */
                    reason: string;
                    /**
                     * BLAKE3 of canonical JSON for the terminal mode snapshot.
                     */
                    terminal_modes_hash: string;
                  }
                | {
                    boundaries: ActorBoundaries3;
                    /**
                     * Number of bytes written completely.
                     */
                    bytes_written: number;
                    /**
                     * Exact bytes emitted by the actor.
                     */
                    data: string;
                    /**
                     * Zero-based exclusive matched column.
                     */
                    end_column: number;
                    /**
                     * Canonical intent event sequence.
                     */
                    intent_event_seq: number;
                    kind: "activated";
                    /**
                     * Canonical operation identifier.
                     */
                    operation_id: string;
                    /**
                     * Presented-state hash used for resolution and encoding.
                     */
                    presented_state_hash: string;
                    /**
                     * Canonical result event sequence.
                     */
                    result_event_seq: number;
                    /**
                     * Zero-based matched row.
                     */
                    row: number;
                    /**
                     * Zero-based first matched column.
                     */
                    start_column: number;
                    /**
                     * Cell chosen for the click.
                     */
                    target_column: number;
                    /**
                     * Exact matched text.
                     */
                    text: string;
                  }
                | {
                    boundaries: ActorBoundaries4;
                    kind: "status";
                    /**
                     * Child process identifier when available.
                     */
                    pid?: number | null;
                  }
                | {
                    boundaries: ActorBoundaries5;
                    /**
                     * Terminal columns.
                     */
                    cols: number;
                    /**
                     * Cursor column when it lies in the viewport.
                     */
                    cursor_column?: number | null;
                    /**
                     * Cursor row when it lies in the viewport.
                     */
                    cursor_row?: number | null;
                    /**
                     * Whether the cursor is visible.
                     */
                    cursor_visible: boolean;
                    kind: "snapshot";
                    modes: ModeState;
                    /**
                     * Session-monotonic observation timestamp.
                     */
                    observed_monotonic_ns: number;
                    /**
                     * Always true for this presentation-filtered endpoint.
                     */
                    presented: boolean;
                    /**
                     * Terminal rows.
                     */
                    rows: number;
                    /**
                     * Zstandard-compressed canonical JSON for the complete semantic state.
                     */
                    semantic_state_zstd: string;
                    /**
                     * BLAKE3 of canonical JSON for `state`.
                     */
                    state_hash: string;
                    /**
                     * Plain text of the presentation-eligible viewport.
                     */
                    viewport_text: string;
                  }
                | {
                    boundaries: ActorBoundaries6;
                    /**
                     * Terminal columns.
                     */
                    cols: number;
                    /**
                     * Cursor column when it lies in the viewport.
                     */
                    cursor_column?: number | null;
                    /**
                     * Cursor row when it lies in the viewport.
                     */
                    cursor_row?: number | null;
                    /**
                     * Whether the cursor is visible.
                     */
                    cursor_visible: boolean;
                    /**
                     * First actor timestamp at which this exact state was observed.
                     */
                    first_observed_monotonic_ns: number;
                    kind: "stable_snapshot";
                    modes: ModeState1;
                    /**
                     * Actor timestamp of the returned snapshot.
                     */
                    observed_monotonic_ns: number;
                    /**
                     * Typed reason the operation returned.
                     */
                    quality: "stable" | "child_completed" | "output_closed" | "deadline";
                    /**
                     * Terminal rows.
                     */
                    rows: number;
                    /**
                     * Zstandard-compressed canonical JSON for the exact semantic state.
                     */
                    semantic_state_zstd: string;
                    /**
                     * Requested unchanged interval.
                     */
                    stable_for_ns: number;
                    /**
                     * BLAKE3 of canonical JSON for this exact state.
                     */
                    state_hash: string;
                    /**
                     * Plain text for the exact presentation-eligible viewport.
                     */
                    viewport_text: string;
                  }
                | {
                    boundaries: ActorBoundaries7;
                    /**
                     * Whether immutable child completion made the final matching state stable.
                     */
                    child_completed: boolean;
                    /**
                     * Terminal columns.
                     */
                    cols: number;
                    /**
                     * `stable` or `matched_unsettled`.
                     */
                    completion_quality: string;
                    /**
                     * First matching timestamp for this unchanged state.
                     */
                    first_match_monotonic_ns: number;
                    kind: "screen_wait";
                    /**
                     * Predicate that matched.
                     */
                    predicate:
                      | {
                          predicate: "contains";
                          /**
                           * Exact UTF-8 substring.
                           */
                          text: string;
                        }
                      | {
                          predicate: "absent";
                          /**
                           * Exact UTF-8 substring.
                           */
                          text: string;
                        }
                      | {
                          /**
                           * Rust-regex pattern, limited by the control service.
                           */
                          pattern: string;
                          predicate: "matches_regex";
                        }
                      | {
                          /**
                           * Rust-regex pattern, limited by the control service.
                           */
                          pattern: string;
                          predicate: "excludes_regex";
                        }
                      | {
                          predicate: "row_contains";
                          /**
                           * Zero-based viewport row.
                           */
                          row: number;
                          /**
                           * Exact UTF-8 substring.
                           */
                          text: string;
                        }
                      | {
                          predicate: "row_absent";
                          /**
                           * Zero-based viewport row.
                           */
                          row: number;
                          /**
                           * Exact UTF-8 substring.
                           */
                          text: string;
                        }
                      | {
                          predicate: "region_contains";
                          region: ScreenRegion;
                          /**
                           * Exact UTF-8 substring.
                           */
                          text: string;
                        }
                      | {
                          predicate: "region_absent";
                          region: ScreenRegion1;
                          /**
                           * Exact UTF-8 substring.
                           */
                          text: string;
                        }
                      | {
                          /**
                           * Optional zero-based viewport column.
                           */
                          column?: number | null;
                          predicate: "cursor";
                          /**
                           * Optional zero-based viewport row.
                           */
                          row?: number | null;
                          /**
                           * Optional cursor shape requirement.
                           */
                          shape?: CursorStyle | null;
                          /**
                           * Optional cursor visibility requirement.
                           */
                          visible?: boolean | null;
                        }
                      | {
                          /**
                           * Required set/reset value.
                           */
                          enabled: boolean;
                          /**
                           * Stable mode selector.
                           */
                          mode:
                            | {
                                mode: "alternate_screen";
                              }
                            | {
                                mode: "bracketed_paste";
                              }
                            | {
                                mode: "wraparound";
                              }
                            | {
                                mode: "synchronized_output";
                              }
                            | {
                                mode: "ansi";
                                /**
                                 * Mode number.
                                 */
                                number: number;
                              }
                            | {
                                mode: "dec";
                                /**
                                 * Mode number.
                                 */
                                number: number;
                              };
                          predicate: "mode";
                        }
                      | {
                          /**
                           * Equality relationship to require.
                           */
                          comparison: "unchanged" | "changed";
                          /**
                           * Lowercase BLAKE3 state hash.
                           */
                          hash: string;
                          predicate: "state_hash";
                        }
                      | {
                          predicate: "all";
                          /**
                           * Bounded non-empty predicate list.
                           */
                          predicates: ScreenPredicate[];
                        }
                      | {
                          predicate: "any";
                          /**
                           * Bounded non-empty predicate list.
                           */
                          predicates: ScreenPredicate[];
                        };
                    /**
                     * Terminal rows.
                     */
                    rows: number;
                    /**
                     * Requested unchanged interval.
                     */
                    stable_for_ns: number;
                    /**
                     * Last timestamp through which the state remained stable.
                     */
                    stable_through_monotonic_ns: number;
                    /**
                     * Full presentation-eligible semantic state hash.
                     */
                    state_hash: string;
                  }
                | {
                    boundaries: ActorBoundaries8;
                    /**
                     * Canonical event sequence containing the marker.
                     */
                    event_seq: number;
                    kind: "marked";
                    /**
                     * Normalized marker label.
                     */
                    label: string;
                  };
              kind: "control";
            }
          | {
              kind: "output_wait";
              outcome: WaitOutcome;
            }
          | {
              completion: SessionCompletion;
              kind: "completion";
            }
          | {
              kind: "observation";
              observation: Observation;
            }
          | {
              /**
               * Complete requested entries.
               */
              entries: TimelineEntry[];
              kind: "timeline";
            }
          | {
              difference: SemanticDiff;
              kind: "diff";
            }
          | {
              kind: "diagnose";
              report: DiagnoseReport;
            }
          | {
              /**
               * Ordered event projections.
               */
              events: EventProjection[];
              kind: "events";
              /**
               * Next event sequence when more source may exist.
               */
              next_event_seq: number;
            }
          | {
              /**
               * `semantic_screenshot` or `native_screenshot`.
               */
              artifact_type: string;
              /**
               * Atomically committed artifact directory.
               */
              directory: string;
              kind: "artifact";
              /**
               * Manifest path.
               */
              manifest: string;
              /**
               * Primary PNG member.
               */
              primary: string;
              /**
               * Exact truth-layer label.
               */
              truth_layer: string;
            }
          | {
              kind: "verification";
              report: VerificationReport;
            }
          | {
              /**
               * Atomically committed directory.
               */
              directory: string;
              kind: "bundle";
              manifest: BundleManifest;
            }
          | {
              kind: "detached";
            }
          | {
              /**
               * Confirms that the canonical directory was removed.
               */
              evidence_removed: boolean;
              kind: "discarded";
            };
        status: "ok";
      }
    | {
        /**
         * Machine-readable code.
         */
        code: string;
        /**
         * Canonical evidence path when known.
         */
        evidence?: string | null;
        /**
         * Human-readable diagnostic.
         */
        message: string;
        /**
         * `safe_same_request_id`, `safe_new_request`, or `unknown`.
         */
        retry_safety: string;
        status: "error";
        /**
         * Whether mutation application may be uncertain.
         */
        uncertain: boolean;
      };
  /**
   * Driver protocol version.
   */
  version: number;
}
/**
 * Durable and observed canonical positions after the operation.
 */
export interface ActorBoundaries {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Latest boundary whose file synchronization completed successfully.
 */
export interface CanonicalBoundary {
  /**
   * Hash of the preceding event, or the event header at sequence zero.
   */
  event_hash: string;
  /**
   * Sequence number that will be assigned to the next event.
   */
  next_event_seq: number;
  /**
   * PTY output bytes included through this boundary.
   */
  output_bytes: number;
}
/**
 * Latest boundary processed by the session actor, possibly volatile.
 */
export interface CanonicalBoundary1 {
  /**
   * Hash of the preceding event, or the event header at sequence zero.
   */
  event_hash: string;
  /**
   * Sequence number that will be assigned to the next event.
   */
  next_event_seq: number;
  /**
   * PTY output bytes included through this boundary.
   */
  output_bytes: number;
}
/**
 * Durable and observed positions after application.
 */
export interface ActorBoundaries1 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Durable and observed positions at evaluation.
 */
export interface ActorBoundaries2 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Durable and observed positions after application.
 */
export interface ActorBoundaries3 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Durable and observed canonical positions at status evaluation.
 */
export interface ActorBoundaries4 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Durable and observed source boundaries at observation time.
 */
export interface ActorBoundaries5 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Presentation-eligible terminal modes.
 */
export interface ModeState {
  /**
   * Any alternate-screen mode.
   */
  alternate_screen: boolean;
  /**
   * DEC bracketed-paste mode 2004.
   */
  bracketed_paste: boolean;
  /**
   * DEC synchronized-output mode 2026.
   */
  synchronized_output: boolean;
  /**
   * Complete stable inventory of known ANSI and DEC modes.
   */
  values: TerminalMode[];
  /**
   * DEC auto-wrap mode 7.
   */
  wraparound: boolean;
  [k: string]: unknown;
}
/**
 * One ANSI or DEC mode value.
 */
export interface TerminalMode {
  /**
   * `true` for ANSI mode, `false` for DEC-private mode.
   */
  ansi: boolean;
  /**
   * Current set/reset value.
   */
  enabled: boolean;
  /**
   * Numeric mode identifier.
   */
  number: number;
  [k: string]: unknown;
}
/**
 * Actor-consistent canonical source boundaries for this exact state.
 */
export interface ActorBoundaries6 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Presentation-eligible terminal modes from this exact state.
 */
export interface ModeState1 {
  /**
   * Any alternate-screen mode.
   */
  alternate_screen: boolean;
  /**
   * DEC bracketed-paste mode 2004.
   */
  bracketed_paste: boolean;
  /**
   * DEC synchronized-output mode 2026.
   */
  synchronized_output: boolean;
  /**
   * Complete stable inventory of known ANSI and DEC modes.
   */
  values: TerminalMode[];
  /**
   * DEC auto-wrap mode 7.
   */
  wraparound: boolean;
  [k: string]: unknown;
}
/**
 * Actor-consistent canonical source boundaries.
 */
export interface ActorBoundaries7 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * Region bounds in viewport cells.
 */
export interface ScreenRegion {
  /**
   * Exclusive bottom row.
   */
  bottom: number;
  /**
   * Inclusive zero-based left column.
   */
  left: number;
  /**
   * Exclusive right column.
   */
  right: number;
  /**
   * Inclusive zero-based top row.
   */
  top: number;
}
/**
 * Region bounds in viewport cells.
 */
export interface ScreenRegion1 {
  /**
   * Exclusive bottom row.
   */
  bottom: number;
  /**
   * Inclusive zero-based left column.
   */
  left: number;
  /**
   * Exclusive right column.
   */
  right: number;
  /**
   * Inclusive zero-based top row.
   */
  top: number;
}
/**
 * Durable and observed canonical positions after the marker.
 */
export interface ActorBoundaries8 {
  durable: CanonicalBoundary;
  observed: CanonicalBoundary1;
}
/**
 * First match and its canonical position.
 */
export interface WaitOutcome {
  /**
   * Event containing the end of the first observed match.
   */
  event_seq: number;
  /**
   * Monotonic timestamp of the containing event.
   */
  monotonic_ns: number;
  /**
   * End offset in the concatenated output stream.
   */
  output_end_offset: number;
  [k: string]: unknown;
}
/**
 * Validated completion envelope.
 */
export interface SessionCompletion {
  durability: DurabilityRecord;
  /**
   * Number of canonical events.
   */
  event_count: number;
  /**
   * BLAKE3 of the exact event-log header bytes.
   */
  event_header_hash: string;
  /**
   * Normal exit code when available.
   */
  exit_code?: number | null;
  /**
   * Hash of the final canonical event.
   */
  final_event_hash: string;
  /**
   * Stable format identifier.
   */
  format: string;
  /**
   * Total raw output bytes.
   */
  output_bytes: number;
  /**
   * Resource limit that caused host termination, when one was reached.
   */
  resource_limit?: ResourceLimitOutcome | null;
  /**
   * Session identifier.
   */
  session_id: string;
  /**
   * BLAKE3 of the exact immutable session manifest bytes.
   */
  session_manifest_hash: string;
  /**
   * Signal name when available.
   */
  signal_name?: string | null;
  /**
   * Sequence number of the required terminal `Exit` event.
   */
  terminal_event_seq: number;
  /**
   * Validation contract applied before publication.
   */
  validation_version: number;
  /**
   * Completion schema version.
   */
  version: number;
}
/**
 * Recorder-observed persistence behavior and final sync result.
 */
export interface DurabilityRecord {
  /**
   * Whether the final event boundary completed file synchronization.
   */
  final_sync_completed: boolean;
  /**
   * Largest PTY byte count pending synchronization.
   */
  maximum_observed_uncommitted_bytes: number;
  /**
   * Largest measured interval with output pending synchronization.
   */
  maximum_observed_uncommitted_ns: number;
  policy: DurabilityPolicy;
  /**
   * Number of successful event-log data synchronizations after header creation.
   */
  sync_count: number;
}
/**
 * Chain-bound configured policy.
 */
export interface DurabilityPolicy {
  /**
   * Maximum uncommitted interval for bounded mode.
   */
  maximum_interval_ms?: number | null;
  /**
   * Maximum uncommitted PTY bytes for bounded mode.
   */
  maximum_uncommitted_bytes?: number | null;
  /**
   * Selected persistence behavior.
   */
  mode: "strict" | "bounded" | "throughput";
}
/**
 * Resource-limit trigger retained in immutable completion evidence.
 */
export interface ResourceLimitOutcome {
  /**
   * Stable limit kind.
   */
  kind: "output_bytes";
  /**
   * Configured trigger threshold.
   */
  limit: number;
  /**
   * Exact observed count when the host triggered termination.
   */
  observed: number;
}
/**
 * Shared observation envelope.
 */
export interface Observation {
  boundary: CanonicalBoundary2;
  /**
   * Exact canonical session path bytes when evidence exists.
   */
  canonical_session?: Base64Bytes | null;
  /**
   * Settlement quality.
   */
  capture_quality: "live" | "stable" | "completed" | "incomplete";
  /**
   * Responsible canonical event sequence when known.
   */
  event_seq?: number | null;
  frame: ObservationFrame;
  /**
   * Integrity relationship to canonical execution evidence.
   */
  integrity: "canonical_chained" | "unchained";
  /**
   * Session-monotonic timestamp when known.
   */
  monotonic_ns?: number | null;
  /**
   * BLAKE3 of canonical JSON for `frame`.
   */
  payload_hash: string;
  /**
   * Whether this state was presentation eligible.
   */
  presented: boolean;
  /**
   * Service path used to resolve this state.
   */
  resolution: "actor_snapshot" | "canonical_replay" | "validated_checkpoint" | "ansi_interpretation";
  /**
   * Canonical session ID when evidence exists.
   */
  session_id?: string | null;
  /**
   * Whether canonical execution evidence was durably completed.
   */
  source_completed: boolean;
  /**
   * Source category.
   */
  source_kind:
    "live_session" | "completed_session" | "interrupted_session" | "ansi_stream" | "imported_ansi" | "one_off_command";
  /**
   * BLAKE3 of the complete semantic state.
   */
  state_hash: string;
  /**
   * Semantic state ID when replay assigned one.
   */
  state_id?: number | null;
  /**
   * Requested state point.
   */
  state_point:
    | {
        selector: "latest_presented";
      }
    | {
        selector: "state_id";
        /**
         * Exact semantic state identifier.
         */
        state_id: number;
      }
    | {
        /**
         * Canonical event sequence.
         */
        event_seq: number;
        selector: "event_seq";
      }
    | {
        /**
         * Session-monotonic nanoseconds.
         */
        monotonic_ns: number;
        selector: "monotonic_ns";
      }
    | {
        /**
         * Unique marker label.
         */
        label: string;
        selector: "marker";
      };
  /**
   * Rendering truth layer.
   */
  truth_layer: "semantic" | "native";
}
/**
 * Source boundary observed or validated.
 */
export interface CanonicalBoundary2 {
  /**
   * Hash of the preceding event, or the event header at sequence zero.
   */
  event_hash: string;
  /**
   * Sequence number that will be assigned to the next event.
   */
  next_event_seq: number;
  /**
   * PTY output bytes included through this boundary.
   */
  output_bytes: number;
}
/**
 * Compact stable frame.
 */
export interface ObservationFrame {
  /**
   * Terminal columns.
   */
  cols: number;
  /**
   * Cursor column when it lies in the viewport.
   */
  cursor_column?: number | null;
  /**
   * Cursor row when it lies in the viewport.
   */
  cursor_row?: number | null;
  /**
   * Whether the cursor is visible.
   */
  cursor_visible: boolean;
  modes: ModeState2;
  /**
   * Terminal rows.
   */
  rows: number;
  /**
   * Plain text for the complete viewport.
   */
  viewport_text: string;
}
/**
 * Stable terminal-mode facts.
 */
export interface ModeState2 {
  /**
   * Any alternate-screen mode.
   */
  alternate_screen: boolean;
  /**
   * DEC bracketed-paste mode 2004.
   */
  bracketed_paste: boolean;
  /**
   * DEC synchronized-output mode 2026.
   */
  synchronized_output: boolean;
  /**
   * Complete stable inventory of known ANSI and DEC modes.
   */
  values: TerminalMode[];
  /**
   * DEC auto-wrap mode 7.
   */
  wraparound: boolean;
  [k: string]: unknown;
}
/**
 * One ordered, source-bound timeline item.
 */
export interface TimelineEntry {
  /**
   * Responsible canonical event, absent for the initial semantic state.
   */
  event_seq?: number | null;
  /**
   * Timeline item payload.
   */
  item:
    | {
        /**
         * Number of complete semantic cell values changed.
         */
        changed_cells: number;
        /**
         * Rows whose semantic cells changed.
         */
        changed_rows: number[];
        /**
         * Whether cursor state changed.
         */
        cursor_changed: boolean;
        /**
         * Byte offset within the responsible output event.
         */
        event_byte_offset?: number | null;
        /**
         * Ghostty's dirty classification.
         */
        ghostty_dirty: "clean" | "partial" | "full";
        kind: "semantic_state";
        /**
         * Whether terminal metadata changed.
         */
        metadata_changed: boolean;
        /**
         * Whether terminal modes changed.
         */
        modes_changed: boolean;
        /**
         * Whether the state was eligible for presentation.
         */
        presented: boolean;
        /**
         * Complete normalized semantic state hash.
         */
        state_hash: string;
        /**
         * Dense semantic state identifier.
         */
        state_id: number;
      }
    | {
        /**
         * Exact typed canonical event.
         */
        event:
          | {
              /**
               * Exact bytes, including control sequences and invalid UTF-8.
               */
              data: string;
              type: "output";
            }
          | {
              /**
               * Exact emitted bytes.
               */
              data: string;
              /**
               * Structured semantic source of the exact emitted bytes.
               */
              descriptor:
                | {
                    kind: "raw";
                  }
                | {
                    kind: "text";
                  }
                | {
                    /**
                     * Alt modifier.
                     */
                    alt: boolean;
                    /**
                     * Control modifier.
                     */
                    control: boolean;
                    /**
                     * Pinned encoder identity.
                     */
                    encoder: string;
                    /**
                     * Stable symbolic key spelling.
                     */
                    key: string;
                    kind: "key";
                    /**
                     * Shift modifier.
                     */
                    shift: boolean;
                    /**
                     * Super modifier.
                     */
                    super_key: boolean;
                    /**
                     * BLAKE3 of canonical JSON for the terminal mode snapshot.
                     */
                    terminal_modes_hash: string;
                  }
                | {
                    /**
                     * Whether the actor applied bracketed-paste wrappers.
                     */
                    bracketed: boolean;
                    kind: "paste";
                    /**
                     * BLAKE3 of canonical JSON for the terminal mode snapshot.
                     */
                    terminal_modes_hash: string;
                  }
                | {
                    /**
                     * Pinned encoder identity.
                     */
                    encoder: string;
                    /**
                     * Whether the terminal gained (`true`) or lost (`false`) focus.
                     */
                    focused: boolean;
                    kind: "focus";
                    /**
                     * BLAKE3 of canonical JSON for the terminal mode snapshot.
                     */
                    terminal_modes_hash: string;
                  }
                | {
                    /**
                     * Button name for clicks, otherwise wheel direction.
                     */
                    button: string;
                    /**
                     * Complete click count, or zero for non-click operations.
                     */
                    click_count: number;
                    /**
                     * Terminal columns at encoding time.
                     */
                    cols: number;
                    /**
                     * Zero-based viewport column.
                     */
                    column: number;
                    /**
                     * Pinned encoder identity.
                     */
                    encoder: string;
                    /**
                     * Optional zero-based destination column for a drag.
                     */
                    end_column?: number | null;
                    /**
                     * Optional zero-based destination row for a drag.
                     */
                    end_row?: number | null;
                    kind: "pointer";
                    /**
                     * Signed requested wheel lines, or zero for clicks.
                     */
                    lines: number;
                    /**
                     * `click`, `multi_click`, `drag`, or `scroll`.
                     */
                    operation: string;
                    /**
                     * Total terminal surface height in pixels.
                     */
                    pixel_height: number;
                    /**
                     * Total terminal surface width in pixels.
                     */
                    pixel_width: number;
                    /**
                     * Zero-based viewport row.
                     */
                    row: number;
                    /**
                     * Terminal rows at encoding time.
                     */
                    rows: number;
                    /**
                     * BLAKE3 of canonical JSON for the terminal mode snapshot.
                     */
                    terminal_modes_hash: string;
                  }
                | {
                    /**
                     * Current grid columns.
                     */
                    cols: number;
                    /**
                     * Pinned encoder identity.
                     */
                    encoder: string;
                    /**
                     * Zero-based exclusive matched column.
                     */
                    end_column: number;
                    kind: "activate";
                    /**
                     * Presented-state hash used for resolution and encoding.
                     */
                    presented_state_hash: string;
                    /**
                     * Zero-based matched row.
                     */
                    row: number;
                    /**
                     * Current grid rows.
                     */
                    rows: number;
                    /**
                     * Zero-based first matched column.
                     */
                    start_column: number;
                    /**
                     * Cell chosen for the click.
                     */
                    target_column: number;
                    /**
                     * BLAKE3 of canonical JSON for the terminal mode snapshot.
                     */
                    terminal_modes_hash: string;
                    /**
                     * Exact matched text.
                     */
                    text: string;
                  };
              /**
               * High-level input category.
               */
              input_kind: "raw" | "text" | "keys" | "paste" | "focus" | "pointer";
              /**
               * Optional symbolic intent such as `ctrl-c` or `paste`.
               */
              intent?: string | null;
              /**
               * Identifier paired with the corresponding result.
               */
              operation_id: string;
              /**
               * Input origin.
               */
              source: "human" | "agent" | "host";
              type: "input_intent";
            }
          | {
              /**
               * Bytes written before success or failure.
               */
              bytes_written: number;
              /**
               * Stable diagnostic code on failure.
               */
              error_code?: string | null;
              /**
               * Identifier from the corresponding intent.
               */
              operation_id: string;
              /**
               * Whether the entire byte sequence was applied.
               */
              status: "applied" | "failed";
              type: "input_result";
            }
          | {
              /**
               * Columns after the resize.
               */
              cols: number;
              /**
               * Identifier paired with the corresponding result.
               */
              operation_id: string;
              /**
               * Total pixel height reported to the child.
               */
              pixel_height: number;
              /**
               * Total pixel width reported to the child.
               */
              pixel_width: number;
              /**
               * Rows after the resize.
               */
              rows: number;
              type: "resize_intent";
            }
          | {
              /**
               * Stable diagnostic code on failure.
               */
              error_code?: string | null;
              /**
               * Identifier from the corresponding intent.
               */
              operation_id: string;
              /**
               * Whether the new geometry was applied.
               */
              status: "applied" | "failed";
              type: "resize_result";
            }
          | {
              /**
               * Stable display name such as `SIGINT`.
               */
              name: string;
              /**
               * Platform signal number.
               */
              number: number;
              /**
               * Identifier paired with the corresponding result.
               */
              operation_id: string;
              type: "signal_intent";
            }
          | {
              /**
               * Stable diagnostic code on failure.
               */
              error_code?: string | null;
              /**
               * Identifier from the corresponding intent.
               */
              operation_id: string;
              /**
               * Whether the signal syscall succeeded.
               */
              status: "applied" | "failed";
              type: "signal_result";
            }
          | {
              /**
               * User-provided marker label.
               */
              label: string;
              /**
               * Caller-owned operation identifier used for durable deduplication.
               */
              operation_id: string;
              type: "marker";
            }
          | {
              /**
               * Query bytes emitted by the child.
               */
              query: string;
              /**
               * Component that generated the response.
               */
              responder: string;
              /**
               * Response bytes sent back to the child.
               */
              response: string;
              type: "query_response";
            }
          | {
              /**
               * Transition kind.
               */
              lifecycle_kind: "spawned" | "output_closed" | "stopping";
              /**
               * Child process ID when available.
               */
              pid?: number | null;
              type: "lifecycle";
            }
          | {
              /**
               * Exit status when the process exited normally.
               */
              code?: number | null;
              /**
               * Signal number when signal termination occurred.
               */
              signal?: number | null;
              /**
               * Platform signal description when available.
               */
              signal_name?: string | null;
              type: "exit";
            }
          | {
              /**
               * Stable diagnostic code.
               */
              code: string;
              /**
               * Redacted human-readable message.
               */
              message: string;
              type: "host_error";
            };
        kind: "canonical_event";
      }
    | {
        kind: "vt_token";
        token: VtToken;
      };
  /**
   * Canonical session offset.
   */
  monotonic_ns: number;
}
/**
 * Exact token and canonical source segments.
 */
export interface VtToken {
  /**
   * Stable neutral description for common controls.
   */
  description: string;
  /**
   * Framed sequence family.
   */
  kind: "printable" | "c0" | "c1" | "esc" | "csi" | "osc" | "dcs" | "apc" | "unknown" | "incomplete";
  /**
   * Exact bytes, including introducer and terminator.
   */
  raw: string;
  /**
   * Ordered exact source segments; concatenating them reproduces `raw`.
   */
  source: VtSourceSegment[];
  /**
   * Timing resolution; current recordings timestamp complete PTY reads.
   */
  timing_resolution: string;
  /**
   * Dense token identifier.
   */
  token_id: number;
}
/**
 * Half-open byte range within one canonical output event.
 */
export interface VtSourceSegment {
  /**
   * Exclusive byte offset within the event payload.
   */
  end_byte_offset: number;
  /**
   * Canonical output event sequence.
   */
  event_seq: number;
  /**
   * Session-monotonic event timestamp.
   */
  monotonic_ns: number;
  /**
   * Inclusive byte offset within the event payload.
   */
  start_byte_offset: number;
}
/**
 * Source-bound semantic difference.
 */
export interface SemanticDiff {
  /**
   * Active SGR style change.
   */
  active_style?: ValueChange5 | null;
  /**
   * Whether any normalized semantic or presentation value changed.
   */
  changed: boolean;
  /**
   * Palette/default-color change.
   */
  colors?: ValueChange4 | null;
  /**
   * Cursor change.
   */
  cursor?: ValueChange2 | null;
  from: DiffEndpoint;
  /**
   * Geometry change.
   */
  geometry?: ValueChange | null;
  /**
   * OSC metadata change.
   */
  metadata?: ValueChange6 | null;
  /**
   * Terminal mode change.
   */
  modes?: ValueChange3 | null;
  /**
   * Presentation-eligibility change.
   */
  presented?: ValueChange7 | null;
  /**
   * Minimal changed semantic cells grouped by row.
   */
  rows: RowChange[];
  to: DiffEndpoint1;
}
/**
 * Before/after values for one changed semantic property.
 */
export interface ValueChange5 {
  after: CellStyle;
  before: CellStyle1;
}
/**
 * Value at the later endpoint.
 */
export interface CellStyle {
  /**
   * Background color source.
   */
  background:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  /**
   * Blink attribute.
   */
  blink: boolean;
  /**
   * Bold attribute.
   */
  bold: boolean;
  /**
   * Faint attribute.
   */
  faint: boolean;
  /**
   * Foreground color source.
   */
  foreground:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  /**
   * Inverse-video attribute.
   */
  inverse: boolean;
  /**
   * Invisible attribute.
   */
  invisible: boolean;
  /**
   * Italic attribute.
   */
  italic: boolean;
  /**
   * Overline attribute.
   */
  overline: boolean;
  /**
   * Strikethrough attribute.
   */
  strikethrough: boolean;
  /**
   * Upstream SGR underline value (`0` through `5`).
   */
  underline: number;
  /**
   * Underline color source.
   */
  underline_color:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  [k: string]: unknown;
}
/**
 * RGB color independent of libghostty's unstable ABI.
 */
export interface Rgb {
  /**
   * Blue channel.
   */
  b: number;
  /**
   * Green channel.
   */
  g: number;
  /**
   * Red channel.
   */
  r: number;
  [k: string]: unknown;
}
/**
 * Value at the earlier endpoint.
 */
export interface CellStyle1 {
  /**
   * Background color source.
   */
  background:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  /**
   * Blink attribute.
   */
  blink: boolean;
  /**
   * Bold attribute.
   */
  bold: boolean;
  /**
   * Faint attribute.
   */
  faint: boolean;
  /**
   * Foreground color source.
   */
  foreground:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  /**
   * Inverse-video attribute.
   */
  inverse: boolean;
  /**
   * Invisible attribute.
   */
  invisible: boolean;
  /**
   * Italic attribute.
   */
  italic: boolean;
  /**
   * Overline attribute.
   */
  overline: boolean;
  /**
   * Strikethrough attribute.
   */
  strikethrough: boolean;
  /**
   * Upstream SGR underline value (`0` through `5`).
   */
  underline: number;
  /**
   * Underline color source.
   */
  underline_color:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  [k: string]: unknown;
}
/**
 * Before/after values for one changed semantic property.
 */
export interface ValueChange4 {
  after: ColorState;
  before: ColorState1;
}
/**
 * Value at the later endpoint.
 */
export interface ColorState {
  background: Rgb1;
  /**
   * Explicit cursor color.
   */
  cursor?: Rgb | null;
  foreground: Rgb2;
  /**
   * Active 256-color palette.
   */
  palette: Rgb[];
  [k: string]: unknown;
}
/**
 * RGB color independent of libghostty's unstable ABI.
 */
export interface Rgb1 {
  /**
   * Blue channel.
   */
  b: number;
  /**
   * Green channel.
   */
  g: number;
  /**
   * Red channel.
   */
  r: number;
  [k: string]: unknown;
}
/**
 * RGB color independent of libghostty's unstable ABI.
 */
export interface Rgb2 {
  /**
   * Blue channel.
   */
  b: number;
  /**
   * Green channel.
   */
  g: number;
  /**
   * Red channel.
   */
  r: number;
  [k: string]: unknown;
}
/**
 * Value at the earlier endpoint.
 */
export interface ColorState1 {
  background: Rgb1;
  /**
   * Explicit cursor color.
   */
  cursor?: Rgb | null;
  foreground: Rgb2;
  /**
   * Active 256-color palette.
   */
  palette: Rgb[];
  [k: string]: unknown;
}
/**
 * Before/after values for one changed semantic property.
 */
export interface ValueChange2 {
  after: CursorState;
  before: CursorState1;
}
/**
 * Value at the later endpoint.
 */
export interface CursorState {
  /**
   * Blink mode.
   */
  blinking: boolean;
  /**
   * Viewport column when available.
   */
  column?: number | null;
  /**
   * Whether the cursor lies in the viewport.
   */
  in_viewport: boolean;
  /**
   * Viewport row when available.
   */
  row?: number | null;
  /**
   * Cursor visual style.
   */
  style: "bar" | "block" | "underline" | "hollow_block";
  /**
   * Visibility mode.
   */
  visible: boolean;
  /**
   * Whether the cursor is on a wide-character tail.
   */
  wide_tail?: boolean | null;
  [k: string]: unknown;
}
/**
 * Value at the earlier endpoint.
 */
export interface CursorState1 {
  /**
   * Blink mode.
   */
  blinking: boolean;
  /**
   * Viewport column when available.
   */
  column?: number | null;
  /**
   * Whether the cursor lies in the viewport.
   */
  in_viewport: boolean;
  /**
   * Viewport row when available.
   */
  row?: number | null;
  /**
   * Cursor visual style.
   */
  style: "bar" | "block" | "underline" | "hollow_block";
  /**
   * Visibility mode.
   */
  visible: boolean;
  /**
   * Whether the cursor is on a wide-character tail.
   */
  wide_tail?: boolean | null;
  [k: string]: unknown;
}
/**
 * Earlier endpoint identity and provenance.
 */
export interface DiffEndpoint {
  /**
   * Responsible event sequence.
   */
  event_seq?: number | null;
  /**
   * Ghostty dirty classification at this endpoint.
   */
  ghostty_dirty: "clean" | "partial" | "full";
  /**
   * Rows Ghostty marked dirty at this endpoint.
   */
  ghostty_dirty_rows: number[];
  /**
   * Session-monotonic timestamp.
   */
  monotonic_ns: number;
  /**
   * Requested endpoint selector.
   */
  point:
    | {
        selector: "latest_presented";
      }
    | {
        selector: "state_id";
        /**
         * Exact semantic state identifier.
         */
        state_id: number;
      }
    | {
        /**
         * Canonical event sequence.
         */
        event_seq: number;
        selector: "event_seq";
      }
    | {
        /**
         * Session-monotonic nanoseconds.
         */
        monotonic_ns: number;
        selector: "monotonic_ns";
      }
    | {
        /**
         * Unique marker label.
         */
        label: string;
        selector: "marker";
      };
  /**
   * Whether the state was presentation eligible.
   */
  presented: boolean;
  /**
   * Complete semantic state hash.
   */
  state_hash: string;
  /**
   * Dense semantic state identifier.
   */
  state_id: number;
}
/**
 * Before/after values for one changed semantic property.
 */
export interface ValueChange {
  /**
   * Value at the later endpoint.
   *
   * @minItems 2
   * @maxItems 2
   */
  after: [unknown, unknown];
  /**
   * Value at the earlier endpoint.
   *
   * @minItems 2
   * @maxItems 2
   */
  before: [unknown, unknown];
}
/**
 * Before/after values for one changed semantic property.
 */
export interface ValueChange6 {
  after: TerminalMetadata;
  before: TerminalMetadata1;
}
/**
 * Value at the later endpoint.
 */
export interface TerminalMetadata {
  /**
   * Raw title bytes.
   */
  title: string;
  /**
   * Raw working-directory bytes.
   */
  working_directory: string;
  [k: string]: unknown;
}
/**
 * Value at the earlier endpoint.
 */
export interface TerminalMetadata1 {
  /**
   * Raw title bytes.
   */
  title: string;
  /**
   * Raw working-directory bytes.
   */
  working_directory: string;
  [k: string]: unknown;
}
/**
 * Before/after values for one changed semantic property.
 */
export interface ValueChange3 {
  after: ModeState3;
  before: ModeState4;
}
/**
 * Value at the later endpoint.
 */
export interface ModeState3 {
  /**
   * Any alternate-screen mode.
   */
  alternate_screen: boolean;
  /**
   * DEC bracketed-paste mode 2004.
   */
  bracketed_paste: boolean;
  /**
   * DEC synchronized-output mode 2026.
   */
  synchronized_output: boolean;
  /**
   * Complete stable inventory of known ANSI and DEC modes.
   */
  values: TerminalMode[];
  /**
   * DEC auto-wrap mode 7.
   */
  wraparound: boolean;
  [k: string]: unknown;
}
/**
 * Value at the earlier endpoint.
 */
export interface ModeState4 {
  /**
   * Any alternate-screen mode.
   */
  alternate_screen: boolean;
  /**
   * DEC bracketed-paste mode 2004.
   */
  bracketed_paste: boolean;
  /**
   * DEC synchronized-output mode 2026.
   */
  synchronized_output: boolean;
  /**
   * Complete stable inventory of known ANSI and DEC modes.
   */
  values: TerminalMode[];
  /**
   * DEC auto-wrap mode 7.
   */
  wraparound: boolean;
  [k: string]: unknown;
}
/**
 * Before/after values for one changed semantic property.
 */
export interface ValueChange7 {
  /**
   * Value at the later endpoint.
   */
  after: boolean;
  /**
   * Value at the earlier endpoint.
   */
  before: boolean;
}
/**
 * Changed cells for one viewport row.
 */
export interface RowChange {
  /**
   * Complete semantic cell changes.
   */
  cells: CellChange[];
  /**
   * Zero-based row.
   */
  row: number;
}
/**
 * One changed viewport cell.
 */
export interface CellChange {
  /**
   * Complete later cell, absent outside later geometry.
   */
  after?: CellState | null;
  /**
   * Complete earlier cell, absent outside earlier geometry.
   */
  before?: CellState | null;
  /**
   * Zero-based column.
   */
  column: number;
}
/**
 * One visible cell in a semantic snapshot.
 */
export interface CellState {
  /**
   * Zero-based column.
   */
  column: number;
  /**
   * Full UTF-8 grapheme cluster; empty for a blank cell.
   */
  grapheme: string;
  /**
   * Resolved explicit background, before terminal-default fallback.
   */
  resolved_background?: Rgb | null;
  /**
   * Resolved explicit foreground, before terminal-default fallback.
   */
  resolved_foreground?: Rgb | null;
  /**
   * Whether the cell is selected.
   */
  selected: boolean;
  style: CellStyle2;
  [k: string]: unknown;
}
/**
 * Visual style.
 */
export interface CellStyle2 {
  /**
   * Background color source.
   */
  background:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  /**
   * Blink attribute.
   */
  blink: boolean;
  /**
   * Bold attribute.
   */
  bold: boolean;
  /**
   * Faint attribute.
   */
  faint: boolean;
  /**
   * Foreground color source.
   */
  foreground:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  /**
   * Inverse-video attribute.
   */
  inverse: boolean;
  /**
   * Invisible attribute.
   */
  invisible: boolean;
  /**
   * Italic attribute.
   */
  italic: boolean;
  /**
   * Overline attribute.
   */
  overline: boolean;
  /**
   * Strikethrough attribute.
   */
  strikethrough: boolean;
  /**
   * Upstream SGR underline value (`0` through `5`).
   */
  underline: number;
  /**
   * Underline color source.
   */
  underline_color:
    | {
        kind: "none";
        [k: string]: unknown;
      }
    | {
        kind: "palette";
        value: number;
        [k: string]: unknown;
      }
    | {
        kind: "rgb";
        value: Rgb;
        [k: string]: unknown;
      };
  [k: string]: unknown;
}
/**
 * Later endpoint identity and provenance.
 */
export interface DiffEndpoint1 {
  /**
   * Responsible event sequence.
   */
  event_seq?: number | null;
  /**
   * Ghostty dirty classification at this endpoint.
   */
  ghostty_dirty: "clean" | "partial" | "full";
  /**
   * Rows Ghostty marked dirty at this endpoint.
   */
  ghostty_dirty_rows: number[];
  /**
   * Session-monotonic timestamp.
   */
  monotonic_ns: number;
  /**
   * Requested endpoint selector.
   */
  point:
    | {
        selector: "latest_presented";
      }
    | {
        selector: "state_id";
        /**
         * Exact semantic state identifier.
         */
        state_id: number;
      }
    | {
        /**
         * Canonical event sequence.
         */
        event_seq: number;
        selector: "event_seq";
      }
    | {
        /**
         * Session-monotonic nanoseconds.
         */
        monotonic_ns: number;
        selector: "monotonic_ns";
      }
    | {
        /**
         * Unique marker label.
         */
        label: string;
        selector: "marker";
      };
  /**
   * Whether the state was presentation eligible.
   */
  presented: boolean;
  /**
   * Complete semantic state hash.
   */
  state_hash: string;
  /**
   * Dense semantic state identifier.
   */
  state_id: number;
}
/**
 * Source-bound findings.
 */
export interface DiagnoseReport {
  /**
   * Minimum changed-cell fraction used for broad redraw findings.
   */
  broad_change_fraction: string;
  /**
   * Ordered detector selection.
   */
  checks: DiagnosticCheck[];
  /**
   * Deterministic findings.
   */
  findings: DiagnosticFinding[];
  /**
   * Stable report format.
   */
  format: string;
  /**
   * Canonical session identity.
   */
  session_id: string;
  /**
   * Canonical event count anchoring the report.
   */
  source_event_count: number;
  /**
   * Final canonical event hash anchoring the report.
   */
  source_final_event_hash: string;
  /**
   * Report schema version.
   */
  version: number;
}
/**
 * One conservative session-wide diagnostic finding.
 */
export interface DiagnosticFinding {
  /**
   * Stable machine-readable finding code.
   */
  code: string;
  /**
   * Conservative confidence label.
   */
  confidence: "high" | "medium";
  /**
   * Exact neutral facts and limitations.
   */
  facts: string[];
  /**
   * Native relationships, if independently available.
   */
  native_correlations: NativeCorrelation[];
  /**
   * Supporting semantic state IDs.
   */
  state_ids: number[];
  /**
   * Supporting VT token IDs.
   */
  token_ids: number[];
}
/**
 * One source-bound relationship to a native artifact.
 */
export interface NativeCorrelation {
  /**
   * Nominal encoded frame when clock mapping supplied one.
   */
  frame?: number | null;
  /**
   * Honest evidence level.
   */
  level: "presentation_eligible" | "scheduled_for_frame" | "post_endpoint_stable_capture" | "frame_observed";
  /**
   * Exact limitation on the relationship.
   */
  limitation: string;
  /**
   * Relative native manifest path.
   */
  manifest: string;
  /**
   * Related semantic state.
   */
  state_id: number;
}
/**
 * Stable JSONL projection of one canonical event record.
 */
export interface EventProjection {
  /**
   * Hash-and-length summaries when byte fields were elided.
   */
  byte_payloads: BytePayloadSummary[];
  /**
   * Exact typed event when byte payloads were retained.
   */
  event?: Event | null;
  /**
   * Stable event variant name.
   */
  event_type: string;
  /**
   * Canonical event hash.
   */
  hash: string;
  /**
   * Non-byte event fields when exact payloads were elided.
   */
  metadata?: {
    [k: string]: unknown;
  };
  /**
   * Whether exact byte fields were elided.
   */
  metadata_only: boolean;
  /**
   * Session-monotonic event timestamp.
   */
  monotonic_ns: number;
  /**
   * Canonical event sequence.
   */
  seq: number;
}
/**
 * Hash-and-length summary for one elided canonical byte field.
 */
export interface BytePayloadSummary {
  /**
   * BLAKE3 of the exact bytes.
   */
  blake3: string;
  /**
   * Exact byte count.
   */
  byte_count: number;
  /**
   * Event field name (`data`, `query`, or `response`).
   */
  field: string;
}
/**
 * Stable complete report, including invalid findings.
 */
export interface VerificationReport {
  /**
   * Whether artifact authenticity was established from a trusted checkpoint.
   */
  artifact_authenticity_established: boolean;
  /**
   * Number of locally cataloged artifact roots verified at content level.
   */
  artifact_roots_verified: number;
  /**
   * Final canonical event hash, when readable.
   */
  final_event_hash?: string | null;
  /**
   * Stable findings; empty when verification succeeds.
   */
  findings: VerificationFinding[];
  /**
   * Stable report format identifier.
   */
  format: string;
  /**
   * Requested verification depth.
   */
  level: "structure" | "content" | "durability_record" | "signature";
  /**
   * Journal-only diagnostic when acceleration is invalid.
   */
  mutation_journal_error?: string | null;
  /**
   * Number of mutation outcomes verified against canonical ranges.
   */
  mutation_journal_records_verified: number;
  /**
   * State of the disposable mutation deduplication acceleration journal.
   */
  mutation_journal_status: "missing" | "verified" | "invalid";
  /**
   * Canonical session directory checked by the verifier.
   */
  session: string;
  /**
   * Whether canonical session authenticity was established from a trusted receipt.
   */
  session_authenticity_established: boolean;
  /**
   * Immutable session identifier, when readable.
   */
  session_id?: string | null;
  /**
   * Exact session-manifest hash, when readable.
   */
  session_manifest_hash?: string | null;
  /**
   * Whether the detached signature matches the exact receipt payload.
   */
  signature_cryptographic_valid: boolean;
  /**
   * Whether a detached evidence receipt was present.
   */
  signature_receipt_present: boolean;
  /**
   * Signer identity asserted by the detached receipt.
   */
  signature_signer_key_id?: string | null;
  /**
   * Whether the configured trust policy accepts the signer identity/key.
   */
  signature_signer_trusted: boolean;
  /**
   * Whether every invariant at the requested level passed.
   */
  valid: boolean;
  /**
   * Report schema version.
   */
  version: number;
}
/**
 * One stable verification diagnostic.
 */
export interface VerificationFinding {
  /**
   * Stable machine-readable finding code.
   */
  code: string;
  /**
   * Human-readable context that is not intended for programmatic matching.
   */
  message: string;
}
/**
 * Complete bundle manifest.
 */
export interface BundleManifest {
  /**
   * Exact source session path bytes.
   */
  canonical_session: string;
  /**
   * Stable manifest format.
   */
  format: string;
  /**
   * Whether canonical raw evidence is included.
   */
  includes_canonical_evidence: boolean;
  /**
   * Whole-file inventory; unmanifested payloads are invalid.
   */
  members: ArtifactMember[];
  /**
   * Selected policy.
   */
  profile: "screen" | "debug" | "forensic";
  /**
   * Immutable canonical session identity.
   */
  session_id: string;
  /**
   * Final canonical event hash.
   */
  source_final_event_hash: string;
  /**
   * Manifest schema version.
   */
  version: number;
}
/**
 * Whole-file integrity metadata embedded in a derived manifest.
 */
export interface ArtifactMember {
  /**
   * BLAKE3 of the complete file.
   */
  blake3: string;
  /**
   * Exact byte length.
   */
  byte_length: number;
  /**
   * Stable media type.
   */
  media_type: string;
  /**
   * Normalized relative path below the artifact directory.
   */
  path: string;
  /**
   * Relationship to the derivation.
   */
  relationship: string;
  /**
   * Public schema identifier, when the file is structured data.
   */
  schema_type?: string | null;
  /**
   * Sensitivity classification.
   */
  sensitivity: "sensitive_terminal_data" | "operational_metadata";
}
