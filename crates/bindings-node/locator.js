'use strict';

// Hand-written AbortSignal facade over the native locator wait. NAPI regeneration never
// touches this file.
function install(binding) {
  const { Locator, CancelToken } = binding;
  const nativeWaitFor = Locator.prototype.waitFor;

  Locator.prototype.waitFor = async function waitFor(options) {
    const { signal, cancel, ...bounds } = options ?? {};
    if (signal !== undefined && !(signal instanceof AbortSignal)) {
      throw new TypeError('signal must be an AbortSignal');
    }
    if (cancel !== undefined && !(cancel instanceof CancelToken)) {
      throw new TypeError('cancel must be a CancelToken');
    }

    const token = cancel ?? new CancelToken();
    const abort = () => token.cancel();
    if (signal?.aborted) {
      abort();
    } else {
      signal?.addEventListener('abort', abort, { once: true });
    }

    try {
      // The native promise settles only after its blocking worker has returned.
      return await nativeWaitFor.call(this, bounds, token);
    } finally {
      signal?.removeEventListener('abort', abort);
    }
  };
}

module.exports = { install };
