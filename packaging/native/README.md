# Native evidence image

This image is the optional Linux GUI/display/codec environment, not a claim that the standalone binary contains a display server or GUI terminal. Builds require three independently verified inputs:

- `BASE_IMAGE` must be an OCI reference pinned by `@sha256:`;
- `dist/termtrace` must be a validated release binary;
- `dist/ghostty.deb` must be the release-pinned GUI package.

Use `scripts/build-native-image.sh`, which rejects tag-only bases and records the source revision, version, and base digest as OCI labels. Package installation follows the documented rebuild policy of the selected immutable base. The release workflow records the resulting image digest and SBOM; native artifact manifests must record that digest when capture runs inside the image.

The default command runs bounded semantic, native screenshot, and presentation capability probes. Supply normal CLI arguments after the image name to run another command. Canonical evidence and artifact destinations must be bind-mounted explicitly and remain owner-protected.
