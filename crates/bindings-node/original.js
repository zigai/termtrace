'use strict';

// AbortSignal startup cancellation for original recordings.
function install(binding) {
  const { Termtrace, OriginalRecording, CancelToken } = binding;
  const nativeRecordOriginal = Termtrace.prototype.recordOriginal;

  Termtrace.prototype.recordOriginal = async function recordOriginal(command, options) {
    const { signal, cancel, ...launch } = options ?? {};
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
      // The native promise settles only after a cancelled capture has been torn down.
      return await nativeRecordOriginal.call(this, command, launch, token);
    } finally {
      signal?.removeEventListener('abort', abort);
    }
  };


}

module.exports = { install };
