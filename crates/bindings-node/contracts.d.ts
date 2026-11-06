export type {
    MediaArtifactResult,
    ArtifactProductKind,
    ArtifactProvenance,
    ArtifactSourceKind,
    ArtifactSourceRef,
    ArtifactManifestRef,
    SdkArtifactMember,
    SdkCanonicalBoundary,
} from "./types";
export declare function decimalUnsigned(value: string, bits?: 64 | 128): bigint;
export declare function timeoutMilliseconds(timeoutMs: number): number;
export declare function validateRegion(
    region: { top: number; left: number; bottom: number; right: number },
    rows: number,
    cols: number,
): void;
