const { existsSync } = require('fs');
const { join } = require('path');

let nativeBinding = null;
const localBinding = join(__dirname, 'termtrace.node');

if (existsSync(localBinding)) {
  nativeBinding = require(localBinding);
} else {
  throw new Error(`Failed to load native binding: ${localBinding}`);
}

// Wire Symbol.asyncDispose to asyncDispose for 'await using' support
if (typeof Symbol.asyncDispose !== 'undefined' && nativeBinding.Recording) {
  nativeBinding.Recording.prototype[Symbol.asyncDispose] = function () {
    return this.asyncDispose();
  };
}

module.exports = nativeBinding;
module.exports.default = nativeBinding;
