/* Generated from schemas/driver-request.schema.json. DO NOT EDIT. */

export type Base64Bytes = string;
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
/**
 * One input atom accepted inside an admission-guarded sequence.
 */
export type InputSequenceAtom =
  | {
      /**
       * Optional caller-supplied reason retained separately from input semantics.
       */
      caller_intent?: string | null;
      /**
       * Bytes encoded as Base64 in JSON.
       */
      data: string;
      input: "bytes";
      /**
       * Semantic category retained in canonical evidence.
       */
      input_kind: "raw" | "text" | "keys" | "paste" | "focus" | "pointer";
      /**
       * Caller-owned operation identifier retained in canonical events.
       */
      operation_id: string;
    }
  | {
      /**
       * Optional caller-supplied reason retained separately from the symbolic key.
       */
      caller_intent?: string | null;
      input: "key";
      /**
       * Layout-independent key.
       */
      key:
        | "enter"
        | "escape"
        | "tab"
        | "backspace"
        | "delete"
        | "insert"
        | "home"
        | "end"
        | "page_up"
        | "page_down"
        | "arrow_up"
        | "arrow_down"
        | "arrow_left"
        | "arrow_right"
        | {
            function: number;
          }
        | {
            letter: string;
          };
      modifiers: KeyModifiers;
      /**
       * Caller-owned operation identifier retained in canonical events.
       */
      operation_id: string;
    }
  | {
      /**
       * Optional caller-supplied reason retained separately from paste semantics.
       */
      caller_intent?: string | null;
      /**
       * Paste content before mode-dependent encoding.
       */
      data: string;
      input: "paste";
      /**
       * Caller-owned operation identifier retained in canonical events.
       */
      operation_id: string;
    };
/**
 * Stable detector selectable by `diagnose --check`.
 */
export type DiagnosticCheck = "full_redraw" | "transient" | "dirty" | "cursor" | "modes" | "palette" | "query_loop";

/**
 * One correlated driver request.
 */
export interface DriverRequest {
  /**
   * Typed driver operation.
   */
  method:
    | {
        method: "hello";
      }
    | {
        /**
         * Initial terminal columns.
         */
        cols: number;
        /**
         * Exact executable and argument bytes.
         */
        command: Base64Bytes[];
        environment?: EnvironmentSpec;
        /**
         * Owner-only root receiving collision-free evidence.
         */
        evidence_root: string;
        method: "launch";
        resource_limits?: ResourceLimits;
        /**
         * Initial terminal rows.
         */
        rows: number;
        /**
         * Optional child working directory.
         */
        working_directory?: string | null;
      }
    | {
        method: "attach";
        /**
         * Name, `name:` reference, `path:` reference, or canonical path.
         */
        session: string;
      }
    | {
        /**
         * Typed actor command.
         */
        command:
          | {
              command: "ping";
            }
          | {
              command: "status";
            }
          | {
              command: "snapshot";
            }
          | {
              command: "wait_screen";
              /**
               * Exact predicate evaluated against the latest presented viewport.
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
               * Required unchanged matching-state interval.
               */
              stable_for_ns: number;
              /**
               * Server-side monotonic deadline interval.
               */
              timeout_ns: number;
            }
          | {
              command: "capture_stable";
              /**
               * Required unchanged presented-state interval.
               */
              stable_for_ns: number;
              /**
               * Server-side monotonic deadline interval.
               */
              timeout_ns: number;
            }
          | {
              /**
               * Optional caller-supplied reason for the operation.
               */
              caller_intent?: string | null;
              command: "input";
              /**
               * Raw bytes encoded as Base64 in JSON.
               */
              data: string;
              /**
               * Semantic category retained in the canonical input intent.
               */
              input_kind: "raw" | "text" | "keys" | "paste" | "focus" | "pointer";
            }
          | {
              /**
               * Ordered input atoms encoded against actor state immediately before each write.
               */
              atoms: InputSequenceAtom[];
              command: "input_sequence";
              /**
               * Retain per-atom identifiers and canonical event positions in the response.
               */
              include_atom_outcomes: boolean;
              /**
               * Delay between atoms, bounded together with the atom count.
               */
              pace_ns: number;
            }
          | {
              /**
               * Ordered input atoms encoded against actor state immediately before each write.
               */
              atoms: InputSequenceAtom[];
              command: "act";
              /**
               * Retain per-atom identifiers and canonical event positions in the response.
               */
              include_atom_outcomes: boolean;
              /**
               * Delay between atoms, bounded together with the atom count.
               */
              pace_ns: number;
              /**
               * Presented-state condition evaluated without releasing actor ordering.
               */
              postcondition:
                | {
                    condition: "none";
                  }
                | {
                    condition: "idle";
                    /**
                     * Required unchanged presented-state interval.
                     */
                    stable_for_ns: number;
                    /**
                     * Server-side monotonic deadline interval.
                     */
                    timeout_ns: number;
                  }
                | {
                    condition: "screen";
                    /**
                     * Exact screen predicate.
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
                     * Server-side monotonic deadline interval.
                     */
                    timeout_ns: number;
                  };
            }
          | {
              command: "key";
              /**
               * Layout-independent key.
               */
              key:
                | "enter"
                | "escape"
                | "tab"
                | "backspace"
                | "delete"
                | "insert"
                | "home"
                | "end"
                | "page_up"
                | "page_down"
                | "arrow_up"
                | "arrow_down"
                | "arrow_left"
                | "arrow_right"
                | {
                    function: number;
                  }
                | {
                    letter: string;
                  };
              modifiers: KeyModifiers1;
            }
          | {
              command: "paste";
              /**
               * Paste content before mode-dependent wrapping or newline conversion.
               */
              data: string;
            }
          | {
              command: "focus";
              /**
               * Whether the terminal gained (`true`) or lost (`false`) focus.
               */
              focused: boolean;
              /**
               * Reject instead of returning a semantic no-op when reporting is disabled.
               */
              strict: boolean;
            }
          | {
              /**
               * Zero-based viewport column.
               */
              column: number;
              command: "pointer";
              /**
               * Click or vertical scrolling plan.
               */
              operation:
                | {
                    /**
                     * Button to click.
                     */
                    button: "left" | "middle" | "right" | "wheel_up" | "wheel_down";
                    kind: "click";
                  }
                | {
                    /**
                     * Button to click.
                     */
                    button: "left" | "middle" | "right" | "wheel_up" | "wheel_down";
                    /**
                     * Number of complete clicks.
                     */
                    count: number;
                    kind: "multi_click";
                  }
                | {
                    /**
                     * Button held throughout the drag.
                     */
                    button: "left" | "middle" | "right" | "wheel_up" | "wheel_down";
                    kind: "drag";
                    /**
                     * Number of motion samples, including the destination.
                     */
                    steps: number;
                    /**
                     * Zero-based destination column.
                     */
                    to_column: number;
                    /**
                     * Zero-based destination row.
                     */
                    to_row: number;
                  }
                | {
                    kind: "scroll";
                    /**
                     * Negative scrolls up and positive scrolls down; zero is invalid.
                     */
                    lines: number;
                  };
              /**
               * Zero-based viewport row.
               */
              row: number;
              /**
               * Reject instead of returning a semantic no-op when reporting is disabled.
               */
              strict: boolean;
            }
          | {
              command: "activate";
              /**
               * Reject instead of returning a semantic no-op when reporting is disabled.
               */
              strict: boolean;
              /**
               * Exact row-contained visible text to resolve.
               */
              text: string;
              /**
               * Require exactly one match instead of choosing the first.
               */
              unique: boolean;
            }
          | {
              /**
               * Terminal columns.
               */
              cols: number;
              command: "resize";
              /**
               * Total pixel height.
               */
              pixel_height: number;
              /**
               * Total pixel width.
               */
              pixel_width: number;
              /**
               * Terminal rows.
               */
              rows: number;
            }
          | {
              command: "signal";
              /**
               * Signal to send.
               */
              signal: "interrupt" | "terminate" | "hangup" | "user1" | "user2";
            }
          | {
              command: "mark";
              /**
               * Non-empty marker label.
               */
              label: string;
            }
          | {
              command: "stop";
            };
        /**
         * Optional optimistic presented-state guard.
         */
        expected_presented_state_hash?: string | null;
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "control";
      }
    | {
        /**
         * Exact byte sequence to find.
         */
        data: string;
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "wait_output";
        /**
         * Bounded wait duration.
         */
        timeout_ms: number;
      }
    | {
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "completion";
      }
    | {
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "complete";
        /**
         * Bounded wait duration.
         */
        timeout_ms: number;
      }
    | {
        /**
         * Process-local owned session handle.
         */
        handle: string;
        method: "discard_completed";
      }
    | {
        /**
         * Process-local session handle.
         */
        handle: string;
        /**
         * Permit explicitly selected hidden synchronized-output state.
         */
        include_hidden: boolean;
        method: "observe";
        /**
         * Exact shared state selector.
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
              selector: "state_hash";
              /**
               * BLAKE3 hash of the complete normalized semantic state.
               */
              state_hash: string;
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
      }
    | {
        /**
         * Include non-presented semantic states.
         */
        all_states: boolean;
        /**
         * Include exact lossless VT frames.
         */
        controls: boolean;
        /**
         * Process-local session handle.
         */
        handle: string;
        /**
         * Maximum returned entries, from 1 through 4096.
         */
        limit: number;
        method: "timeline";
        /**
         * Zero-based entry offset.
         */
        offset: number;
      }
    | {
        /**
         * Earlier state point.
         */
        from:
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
              selector: "state_hash";
              /**
               * BLAKE3 hash of the complete normalized semantic state.
               */
              state_hash: string;
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
         * Process-local session handle.
         */
        handle: string;
        method: "diff";
        /**
         * Later state point.
         */
        to:
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
              selector: "state_hash";
              /**
               * BLAKE3 hash of the complete normalized semantic state.
               */
              state_hash: string;
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
      }
    | {
        /**
         * Broad repaint threshold from 1 through 100.
         */
        broad_change_percent: number;
        /**
         * Empty means every detector.
         */
        checks: DiagnosticCheck[];
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "diagnose";
      }
    | {
        /**
         * Inclusive first event sequence.
         */
        from_event_seq: number;
        /**
         * Process-local session handle.
         */
        handle: string;
        /**
         * Maximum returned events, bounded by the driver.
         */
        limit: number;
        /**
         * Elide exact byte payloads.
         */
        metadata_only: boolean;
        method: "events";
      }
    | {
        /**
         * No-replace artifact directory.
         */
        destination: string;
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "screenshot";
        /**
         * Exact selected state point.
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
              selector: "state_hash";
              /**
               * BLAKE3 hash of the complete normalized semantic state.
               */
              state_hash: string;
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
         * Explicit truth renderer.
         */
        renderer: "semantic" | "native";
        /**
         * Optional strict native viewport; absent runs the solver.
         */
        viewport?: CaptureTarget | null;
      }
    | {
        /**
         * Process-local session handle.
         */
        handle: string;
        /**
         * Requested validation depth.
         */
        level: "structure" | "content" | "durability_record" | "signature";
        method: "verify";
      }
    | {
        /**
         * New no-replace bundle directory.
         */
        destination: string;
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "bundle";
        /**
         * Explicit sensitivity profile.
         */
        profile: "screen" | "debug" | "forensic";
      }
    | {
        /**
         * Process-local session handle.
         */
        handle: string;
        method: "detach";
      };
  /**
   * Caller-owned retry/correlation identifier.
   */
  request_id: string;
  /**
   * Driver protocol version.
   */
  version: number;
}
/**
 * Deterministic child-environment policy; values travel only in driver stdin.
 */
export interface EnvironmentSpec {
  /**
   * Whether resolution begins with the recorder's ambient environment.
   */
  base?: "inherit" | "clear";
  /**
   * Deterministic color-variable behavior.
   */
  color?: "auto" | "always" | "never";
  /**
   * Names copied from the recorder environment without placing values in arguments.
   */
  copy?: string[];
  /**
   * Exact UTF-8-named values supplied by the caller.
   */
  set?: {
    [k: string]: Base64Bytes;
  };
  /**
   * Names removed from an inherited environment.
   */
  unset?: string[];
}
/**
 * Non-truncating runtime resource limits.
 */
export interface ResourceLimits {
  /**
   * Soft output budget that triggers termination after all observed bytes are retained.
   */
  output_bytes?: number | null;
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
 * Typed modifier state.
 */
export interface KeyModifiers {
  /**
   * Alt modifier.
   */
  alt: boolean;
  /**
   * Control modifier.
   */
  control: boolean;
  /**
   * Shift modifier.
   */
  shift: boolean;
  /**
   * Super modifier.
   */
  super_key: boolean;
  [k: string]: unknown;
}
/**
 * Typed modifier state.
 */
export interface KeyModifiers1 {
  /**
   * Alt modifier.
   */
  alt: boolean;
  /**
   * Control modifier.
   */
  control: boolean;
  /**
   * Shift modifier.
   */
  shift: boolean;
  /**
   * Super modifier.
   */
  super_key: boolean;
  [k: string]: unknown;
}
/**
 * Pixel viewport intended for native visual capture.
 */
export interface CaptureTarget {
  /**
   * Viewport height in pixels.
   */
  height: number;
  /**
   * Viewport width in pixels.
   */
  width: number;
  [k: string]: unknown;
}
