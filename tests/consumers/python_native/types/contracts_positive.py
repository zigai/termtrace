from termtrace import MediaArtifactResult, decode_media_artifact, encode_media_artifact
from termtrace.contracts import decimal_unsigned, timeout_seconds

value: MediaArtifactResult = decode_media_artifact('{}')
wide: int = value['members'][0]['byte_length']
raw: bytes = value['directory_bytes']
encoded: str = encode_media_artifact(value)
max128: int = decimal_unsigned('340282366920938463463374607431768211455', 128)
seconds: float = timeout_seconds(timeout_ms=1250)
