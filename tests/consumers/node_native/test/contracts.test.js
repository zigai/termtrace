"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const {
    decimalUnsigned,
    timeoutMilliseconds,
    validateRegion,
} = require("@termtrace/core/contracts");

test("explicit decimal conversion rejects malformed values and overflow", () => {
    assert.equal(decimalUnsigned((2n ** 128n - 1n).toString(), 128), 2n ** 128n - 1n);
    for (const bad of ["01", "-1", "1.5", "", "18446744073709551616", 1, NaN])
        assert.throws(() => decimalUnsigned(bad));
    assert.throws(() => decimalUnsigned((2n ** 128n).toString(), 128));
});
test("explicit units and half-open region validation", () => {
    assert.equal(timeoutMilliseconds(1250), 1250);
    for (const n of [-1, NaN, Infinity]) assert.throws(() => timeoutMilliseconds(n));
    validateRegion({ top: 0, left: 0, bottom: 24, right: 80 }, 24, 80);
    assert.throws(() => validateRegion({ top: 2, left: 0, bottom: 1, right: 80 }, 24, 80));
});
