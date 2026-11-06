const { existsSync } = require("fs");
const { join } = require("path");

let nativeBinding = null;
const localBinding = join(__dirname, "termtrace.node");

if (existsSync(localBinding)) {
    nativeBinding = require(localBinding);
} else {
    throw new Error(`Failed to load native binding: ${localBinding}`);
}

class ExpectationMismatchError extends Error {}

const TYPED_PREFIX = "termtrace:";
const ARGUMENT_PREFIX = "termtrace-argument:";
const ARGUMENT_ERRORS = { TypeError, RangeError };

// Lift the typed cause attached by the native binding into `kind`, `code`, `effect`, and
// `retry` properties on the thrown error.
function typed(error) {
    const cause = error && error.cause;
    if (cause && typeof cause.message === "string" && cause.message.startsWith(ARGUMENT_PREFIX)) {
        const ArgumentError =
            ARGUMENT_ERRORS[cause.message.slice(ARGUMENT_PREFIX.length)] ?? TypeError;
        return new ArgumentError(error.message);
    }
    if (cause && typeof cause.message === "string" && cause.message.startsWith(TYPED_PREFIX)) {
        const payload = nativeBinding.decodeErrorData(cause.message.slice(TYPED_PREFIX.length));
        const { kind, code, effect, retry, details, frame_json: frameJson } = payload;
        if (kind === "expectation_mismatch") {
            Object.setPrototypeOf(error, ExpectationMismatchError.prototype);
            error.name = "ExpectationMismatchError";
        }
        error.kind = kind;
        error.code = code;
        error.effect = effect;
        error.retry = retry;
        error.scope = payload.scope ?? "actor";
        if (payload.management !== undefined) {
            error.guidance = payload.guidance;
            error.management = payload.management;
            error.effect = null;
            error.retry = null;
            if (error.management.operation === "original_capture") {
                const { facts } = error.management;
                error.completion = facts.completion;
                error.reason = facts.reason;
                error.evidence = facts.surviving;
                error.capture_state = facts.state;
                error.cleanup = facts.cleanup;
            }
        }
        if (details !== undefined) error.details = details;
        for (const [name, value] of Object.entries(details ?? {})) {
            if (name !== "kind" && name !== "message") {
                error[name] = value;
            }
        }
        if (frameJson !== undefined) {
            error.frame = frameJson === null ? null : nativeBinding.ScreenFrame.fromJson(frameJson);
        }
        if (payload.request_id !== undefined) {
            error.request_id = payload.request_id;
            error.details = payload.details;
        }
        if (payload.action_result_json !== undefined) {
            error.result = nativeBinding.ActionResult.fromJson(payload.action_result_json);
        }
        if (payload.step !== undefined) {
            error.step = payload.step;
            error.send_request_id = payload.send_request_id;
        }
        delete error.cause;
    }
    return error;
}

function wrapMethods(prototype) {
    for (const name of Object.getOwnPropertyNames(prototype)) {
        const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
        if (name === "constructor" || typeof descriptor.value !== "function") {
            continue;
        }
        const method = descriptor.value;
        prototype[name] = function (...args) {
            let result;
            try {
                result = method.apply(this, args);
            } catch (error) {
                throw typed(error);
            }
            if (result && typeof result.then === "function") {
                return result.catch((error) => {
                    throw typed(error);
                });
            }
            return result;
        };
    }
}

// Native static methods are non-configurable, so a class with statics is exported through a
// constructor that shares its prototype (keeping `instanceof`) and lifts typed errors from its
// static methods too.
function wrapStatics(constructor) {
    const statics = Object.getOwnPropertyNames(constructor).filter(
        (name) => name !== "prototype" && typeof constructor[name] === "function",
    );
    if (statics.length === 0) {
        return constructor;
    }

    function Exported(...args) {
        if (new.target === undefined) {
            throw new TypeError(
                `Class constructor ${constructor.name} cannot be invoked without 'new'`,
            );
        }
        return Reflect.construct(constructor, args);
    }
    Object.defineProperty(Exported, "name", { value: constructor.name });
    Exported.prototype = constructor.prototype;
    for (const name of statics) {
        const method = constructor[name];
        Exported[name] = function (...args) {
            try {
                return method.apply(constructor, args);
            } catch (error) {
                throw typed(error);
            }
        };
    }
    return Exported;
}

for (const [name, exported] of Object.entries(nativeBinding)) {
    if (typeof exported === "function" && exported.prototype) {
        wrapMethods(exported.prototype);
        nativeBinding[name] = wrapStatics(exported);
    }
}

for (const name of [
    "failNegation",
    "normalizedSnapshot",
    "maskedSnapshot",
    "assertSnapshotFile",
    "assertMaskedSnapshotFile",
    "assertScreenText",
    "assertCursor",
    "assertNoWrappedRows",
    "assertNeutralRegion",
    "assertNoPresentedTransient",
    "assertExists",
    "assertNotExists",
    "assertCount",
    "assertMatchText",
    "assertMatchStyle",
    "finalizeRecordings",
    "normalizeEdit",
    "sourceForOutputFrame",
    "outputFramesForSourceFrame",
    "writeScenarioJunit",
    "scenarioCiFailed",
]) {
    const method = nativeBinding[name];
    nativeBinding[name] = function (...args) {
        try {
            const result = method(...args);
            return result && typeof result.then === "function"
                ? result.catch((error) => {
                      throw typed(error);
                  })
                : result;
        } catch (error) {
            throw typed(error);
        }
    };
}
nativeBinding.ExpectationMismatchError = ExpectationMismatchError;
require("./locator.js").install(nativeBinding);
require("./original.js").install(nativeBinding);
require("./edit.js").install(nativeBinding);

// Wire Symbol.asyncDispose to asyncDispose for 'await using' support
if (typeof Symbol.asyncDispose !== "undefined" && nativeBinding.Recording) {
    nativeBinding.Recording.prototype[Symbol.asyncDispose] = function () {
        return this.asyncDispose();
    };
}

module.exports = nativeBinding;
module.exports.default = nativeBinding;

// Static CommonJS names preserve ordinary ESM named imports for the additive namespaces.
module.exports.Events = nativeBinding.Events;
module.exports.Output = nativeBinding.Output;

module.exports.Diagnostics = nativeBinding.Diagnostics;
module.exports.Evidence = nativeBinding.Evidence;
module.exports.Registry = nativeBinding.Registry;

module.exports.Tmux = nativeBinding.Tmux;
module.exports.System = nativeBinding.System;
module.exports.Instrumentation = nativeBinding.Instrumentation;
module.exports.Native = nativeBinding.Native;

module.exports.Edit = nativeBinding.Edit;
module.exports.Selector = nativeBinding.Selector;
module.exports.select = nativeBinding.select;
module.exports.scope = nativeBinding.scope;
module.exports.normalizeEdit = nativeBinding.normalizeEdit;
module.exports.sourceForOutputFrame = nativeBinding.sourceForOutputFrame;
module.exports.outputFramesForSourceFrame = nativeBinding.outputFramesForSourceFrame;
module.exports.writeScenarioJunit = nativeBinding.writeScenarioJunit;
module.exports.scenarioCiFailed = nativeBinding.scenarioCiFailed;
