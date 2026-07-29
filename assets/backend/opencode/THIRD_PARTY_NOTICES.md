# OpenCode runtime notice

Aishe can install and supervise an exact, compatibility-pinned OpenCode
runtime. OpenCode is an independent project distributed under the MIT license.

- Project: https://github.com/anomalyco/opencode
- Pinned version: 1.18.9
- License: https://github.com/anomalyco/opencode/blob/v1.18.9/LICENSE

The OpenCode runtime is not linked into the Aishe binary. Aishe downloads the
platform archive identified in `runtime-manifest.json`, verifies its embedded
SHA-256 digest, and stores it in the invoking user's private Aishe data
directory.
