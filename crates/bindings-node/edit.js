"use strict";

// Typed edit builders. They only hold values and reject wrong JavaScript types; validation,
// subtitle path resolution and the edit_plan_hash come from the one Rust normalizer.

const U64_MAX = (1n << 64n) - 1n;

function u64(value, name) {
    if (typeof value === "number") {
        if (!Number.isSafeInteger(value)) {
            throw new TypeError(`${name} must be a bigint or a safe integer, got ${value}`);
        }
        value = BigInt(value);
    } else if (typeof value !== "bigint") {
        throw new TypeError(`${name} must be a bigint or a safe integer, not ${typeof value}`);
    }
    if (value < 0n || value > U64_MAX) {
        throw new RangeError(`${name} must be in 0..=2**64-1, got ${value}`);
    }
    return value;
}

function integer(value, name) {
    if (typeof value !== "number" || !Number.isSafeInteger(value)) {
        throw new TypeError(`${name} must be an integer number, got ${String(value)}`);
    }
    return value;
}

class Selector {
    constructor(wire) {
        this.wire = Object.freeze(wire);
        Object.freeze(this);
    }

    toWire() {
        return { ...this.wire };
    }
}

const select = Object.freeze({
    marker(label) {
        if (typeof label !== "string") throw new TypeError("marker must be a string");
        return new Selector({ marker: label });
    },
    atNs(offset) {
        return new Selector({ monotonicNs: u64(offset, "monotonicNs") });
    },
    event(seq) {
        return new Selector({ eventSeq: u64(seq, "eventSeq") });
    },
    state(id) {
        return new Selector({ stateId: u64(id, "stateId") });
    },
    nativeFrame(frame) {
        return new Selector({ nativeFrame: u64(frame, "nativeFrame") });
    },
});

function selector(value, name) {
    if (!(value instanceof Selector)) throw new TypeError(`${name} must be a Selector from select`);
    return value.toWire();
}

const scope = Object.freeze({
    wholeVideo() {
        return { scope: "whole_video" };
    },
    clip(index) {
        return { scope: "clip", clip_index: u64(index, "clip_index") };
    },
    outputFrame(frame) {
        return { scope: "output_frame", output_frame: u64(frame, "output_frame") };
    },
    range(from, to) {
        return { scope: "range", from: selector(from, "from"), to: selector(to, "to") };
    },
});

function speed(value) {
    if (value === null || typeof value !== "object") {
        throw new TypeError("speed must be { numerator, denominator }");
    }
    return {
        numerator: integer(value.numerator, "speed numerator"),
        denominator: integer(value.denominator, "speed denominator"),
    };
}

class Edit {
    constructor(options = {}) {
        const { resource_base: resourceBase = null, ...rest } = options;
        const unknown = Object.keys(rest);
        if (unknown.length > 0) throw new TypeError(`unknown Edit option: ${unknown[0]}`);
        this.resource_base = resourceBase;
        this.plan = { version: 2, clips: [] };
    }

    clip(from, to, options = {}) {
        const {
            speed: rate = { numerator: 1, denominator: 1 },
            hold_ms: holdMs = 0n,
            caption = null,
        } = options;
        this.plan.clips.push({
            from: selector(from, "from"),
            to: selector(to, "to"),
            speed: speed(rate),
            hold_ms: u64(holdMs, "hold_ms"),
            caption,
        });
        return this;
    }

    annotate(annotation) {
        const {
            text = null,
            template = null,
            scope: lifetime,
            position,
            style,
            z_order: zOrder = 0,
        } = annotation;
        const entry = { text, template, scope: lifetime, z_order: integer(zOrder, "z_order") };
        if (position !== undefined) entry.position = position;
        if (style !== undefined) entry.style = style;
        (this.plan.annotations ??= []).push(entry);
        return this;
    }

    subtitles(ass) {
        (this.plan.subtitles ??= []).push({ ass });
        return this;
    }

    changes(source, mode, options = {}) {
        const { hold_frames: holdFrames = 0, pixel_threshold: threshold = null } = options;
        this.plan.change_visualization = {
            source,
            mode,
            hold_frames: integer(holdFrames, "hold_frames"),
            pixel_threshold: threshold === null ? null : integer(threshold, "pixel_threshold"),
        };
        return this;
    }

    captionLayout(caption) {
        this.plan.layout = { ...(this.plan.layout ?? { caption: "footer" }), caption };
        return this;
    }

    branding(label) {
        this.plan.layout = { ...(this.plan.layout ?? { caption: "footer" }), branding: label };
        return this;
    }

    delivery({ codec, pixel_format: pixelFormat, crf, encoder_preset: encoderPreset }) {
        this.plan.delivery = {
            codec,
            pixel_format: pixelFormat,
            crf: integer(crf, "crf"),
            encoder_preset: encoderPreset,
        };
        return this;
    }

    cursorMask() {
        return this.transform({ transform: "cursor_mask" });
    }

    crop(x, y, width, height) {
        return this.transform({ transform: "crop", ...rectangle(x, y, width, height) });
    }

    zoom(numerator, denominator) {
        return this.transform({
            transform: "zoom",
            numerator: integer(numerator, "numerator"),
            denominator: integer(denominator, "denominator"),
        });
    }

    resize(width, height) {
        return this.transform({
            transform: "resize",
            width: integer(width, "width"),
            height: integer(height, "height"),
        });
    }

    callout(x, y, width, height, options = {}) {
        const { label = null } = options;
        return this.transform({ transform: "callout", ...rectangle(x, y, width, height), label });
    }

    transform(value) {
        (this.plan.transforms ??= []).push(value);
        return this;
    }

    build() {
        return structuredClone(this.plan);
    }

    toSource() {
        return { kind: "plan", edit: this.build(), resource_base: this.resource_base };
    }

    normalize() {
        return exported.normalizeEdit(this);
    }
}

function rectangle(x, y, width, height) {
    return {
        x: integer(x, "x"),
        y: integer(y, "y"),
        width: integer(width, "width"),
        height: integer(height, "height"),
    };
}

function source(edit) {
    if (edit instanceof Edit) return edit.toSource();
    if (typeof edit === "string" || edit instanceof Uint8Array) return { kind: "file", path: edit };
    return edit;
}

const exported = {};

function install(binding) {
    const { Termtrace } = binding;
    for (const name of ["videoPlan", "videoPreview", "videoRender"]) {
        const native = Termtrace.prototype[name];
        Termtrace.prototype[name] = function (session, edit, ...rest) {
            return native.call(this, session, source(edit), ...rest);
        };
    }

    const nativeNormalize = binding.normalizeEdit;
    binding.normalizeEdit = function normalizeEdit(edit) {
        return nativeNormalize(source(edit));
    };
    exported.normalizeEdit = binding.normalizeEdit;

    binding.Edit = Edit;
    binding.Selector = Selector;
    binding.select = select;
    binding.scope = scope;
}

module.exports = { install, Edit, Selector, select, scope };
