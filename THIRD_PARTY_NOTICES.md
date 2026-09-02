# Third-party notices

Aishe can download and supervise an exact compatibility-pinned OpenCode
runtime. OpenCode is an independent project distributed under the MIT license.
It is not linked into the Aishe binary.

- Project: <https://github.com/anomalyco/opencode>
- Pinned compatibility version: `1.18.27`
- Upstream license: `assets/backend/opencode/LICENSE`
- Runtime manifest and archive digests:
  `assets/backend/opencode/runtime-manifest.json`

The release workflow verifies the pinned upstream license, downloads every
approved platform archive, checks its declared size and SHA-256 digest,
generates an SPDX JSON software bill of materials, and publishes build
provenance before a draft release can become public.

The managed runtime installation also carries its own `LICENSE` and
`THIRD_PARTY_NOTICES.md` inside Aishe's private runtime directory.
