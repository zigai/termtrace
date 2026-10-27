from termtrace import MediaArtifactResult, decode_media_artifact
from termtrace.contracts import decimal_unsigned, timeout_seconds

value: MediaArtifactResult = decode_media_artifact('{}')
value['directory_bytes'] = 'AA=='
value['source']['boundary']['next_event_seq'] = '9007199254740993'
decimal_unsigned('1', 32)
timeout_seconds(timeout_ms='1s')
