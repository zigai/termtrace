const { existsSync } = require('fs');
const { join } = require('path');

let nativeBinding = null;
const localBinding = join(__dirname, 'termtrace.node');

if (existsSync(localBinding)) {
  nativeBinding = require(localBinding);
} else {
  throw new Error(`Failed to load native binding: ${localBinding}`);
}

const TYPED_PREFIX = 'termtrace:';

// Lift the typed cause attached by the native binding into `kind`, `code`, `effect`, and
// `retry` properties on the thrown error.
function typed(error) {
  const cause = error && error.cause;
  if (cause && typeof cause.message === 'string' && cause.message.startsWith(TYPED_PREFIX)) {
    const { kind, code, effect, retry } = JSON.parse(cause.message.slice(TYPED_PREFIX.length));
    error.kind = kind;
    error.code = code;
    error.effect = effect;
    error.retry = retry;
    delete error.cause;
  }
  return error;
}

function wrapMethods(prototype) {
  for (const name of Object.getOwnPropertyNames(prototype)) {
    const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
    if (name === 'constructor' || typeof descriptor.value !== 'function') {
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
      if (result && typeof result.then === 'function') {
        return result.catch((error) => {
          throw typed(error);
        });
      }
      return result;
    };
  }
}

for (const exported of Object.values(nativeBinding)) {
  if (typeof exported === 'function' && exported.prototype) {
    wrapMethods(exported.prototype);
  }
}

// Wire Symbol.asyncDispose to asyncDispose for 'await using' support
if (typeof Symbol.asyncDispose !== 'undefined' && nativeBinding.Recording) {
  nativeBinding.Recording.prototype[Symbol.asyncDispose] = function () {
    return this.asyncDispose();
  };
}

module.exports = nativeBinding;
module.exports.default = nativeBinding;
