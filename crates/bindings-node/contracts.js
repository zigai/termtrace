"use strict";
// Schema-owned codec. Never guess fields in canonical evidence or legacy JSON.
function decimalUnsigned(value, bits = 64) {
    if (![64, 128].includes(bits) || typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value))
        throw new TypeError("expected canonical unsigned decimal string");
    const n = BigInt(value);
    if (n >= 1n << BigInt(bits)) throw new RangeError("unsigned integer overflow");
    return n;
}
function timeoutMilliseconds(timeoutMs) {
    if (
        typeof timeoutMs !== "number" ||
        !Number.isFinite(timeoutMs) ||
        timeoutMs < 0 ||
        timeoutMs / 1000 >= 2 ** 64
    )
        throw new RangeError("invalid duration");
    return timeoutMs;
}
function validateRegion(r, rows, cols) {
    if (
        ![r.top, r.left, r.bottom, r.right, rows, cols].every(Number.isInteger) ||
        !(
            0 <= r.top &&
            r.top < r.bottom &&
            r.bottom <= rows &&
            rows <= 65535 &&
            0 <= r.left &&
            r.left < r.right &&
            r.right <= cols &&
            cols <= 65535
        )
    )
        throw new RangeError("invalid half-open screen region");
}
module.exports = { decimalUnsigned, timeoutMilliseconds, validateRegion };
