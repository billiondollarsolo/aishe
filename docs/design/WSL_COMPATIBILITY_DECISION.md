> **Lifecycle: Active.** Baseline: AIShe v0.6.5 (`4a2c7e4`), 2026-08-01.

# Windows Subsystem for Linux compatibility decision

## Decision

**No dedicated WSL build, package, or supported-tier claim in the current
milestone.** WSL2 remains a research/qualification target. Existing Linux
artifacts may be experimented with inside a distribution, but successful
installation or startup is not a compatibility guarantee. Native Windows
remains unsupported; AIShe does not ship a Windows `.exe` or PowerShell/CMD
integration.

This separates two decisions that must not be conflated:

- a future WSL tier would run the Linux AIShe binary and zsh inside WSL; and
- native Windows would require a different process, PTY, shell, packaging, and
  security architecture and is out of scope.

## Evaluation

### User value

WSL could give Windows developers the Linux/zsh AIShe experience with less
architecture divergence than native Windows. It is therefore worth a later
qualification spike. The project currently lacks an actual WSL runner,
distribution matrix, ConPTY/Windows Terminal evidence, and cross-filesystem
security results, so a support claim now would outrun the evidence.

### Architecture

Microsoft documents material differences between WSL1 and WSL2: WSL2 uses a
managed virtual machine and a full Linux kernel, while WSL1 uses a translation
architecture and does not have equivalent system-call compatibility. Future
qualification should target **WSL2 first**, state exact Windows/WSL/distro
versions, and keep WSL1 unqualified unless separately proven.

Relevant upstream references:

- [Microsoft: comparing WSL versions](https://learn.microsoft.com/en-us/windows/wsl/compare-versions)
- [Microsoft: WSL interoperability](https://learn.microsoft.com/en-us/windows/dev-environment/wsl-interop)
- [Microsoft: working across file systems](https://learn.microsoft.com/en-us/windows/wsl/filesystems)
- [Microsoft: systemd in WSL](https://learn.microsoft.com/en-us/windows/wsl/systemd)

The Linux binary, real zsh, and PTY architecture may be reusable. The unproven
boundaries include ConPTY and Windows Terminal resize/key propagation, WSL
startup/shutdown, localhost forwarding for managed backend/OAuth flows,
systemd-enabled versus WSL-init distributions, path conversion, file metadata,
and invocation of Windows executables from the Linux shell.

### Security and privacy

WSL intentionally supports Linux/Windows interoperation. Microsoft documents
that Windows executables can run from WSL and that Windows drives are normally
mounted below `/mnt`. AIShe must not treat those boundaries as equivalent to an
ordinary Linux-only workspace without qualification.

A support proposal must test and document:

- workspace and host-scope resolution for Linux files, `/mnt/c` and other
  mounted drives, UNC-accessed Linux files, symlinks, metadata, and case rules;
- whether yolo sandboxing and bubblewrap are available and enforce the same
  boundary on each declared distro/WSL version;
- Windows executable interop (`powershell.exe`, `cmd.exe`, and arbitrary
  `.exe` files on Windows PATH) in routing, risk analysis, audit, and approval;
- credential and environment sharing across WSL interop, plus browser/OAuth
  callbacks crossing the VM/host network boundary; and
- cleanup and permissions for config, runtime, session, history, audit, and
  state-handoff files on Linux versus mounted Windows storage.

Failure to sandbox one qualified layout must be visible in setup/Doctor and
cannot silently downgrade a supported safety claim.

### Packaging

No WSL-specific artifact is needed unless qualification finds a real delta:
the likely delivery unit is an existing Linux glibc or musl binary installed
*inside* a distribution. A future support tier must nevertheless define:

- distro/package-manager prerequisites and whether `.deb`, tarball, or installer
  is the recommended path;
- zsh, bubblewrap, managed runtime, CA certificates, and browser dependencies;
- Linux-home versus `/mnt/c` installation and project-location guidance;
- upgrade/rollback behavior while a distribution is stopped; and
- explicit refusal to present that package as a native Windows binary.

The current outcome is therefore **no new build and no new package**.

### Test and maintenance cost

A build/support decision requires real WSL2 evidence, not a Linux container
with environment variables that resemble WSL. The minimum proposed matrix is:

- current supported Windows 11 plus one still-supported Windows 10 baseline;
- WSL2 Ubuntu and one second distribution, with systemd-enabled and WSL-init
  states where applicable;
- x86_64 Linux artifact install, upgrade, rollback, and uninstall inside WSL;
- projects in the Linux filesystem and `/mnt/c`, including spaces, Unicode,
  symlinks, permissions, case collisions, and large repository I/O;
- direct commands, agent routing, suggest staging, picker/setup/status,
  Ctrl-C/Ctrl-Z, 300 ms escape latency, resize, tmux, and SSH;
- Windows Terminal and VS Code/Cursor integrated terminals with recorded
  Windows, WSL, distro, `TERM`, and AIShe identities;
- managed backend install/start/reconnect, localhost/OAuth callback behavior,
  networking changes, WSL restart, and Windows reboot persistence; and
- sandbox/workspace escape probes involving mounted drives and Windows
  executables.

A self-hosted or otherwise genuine Windows+WSL runner is required for the
blocking subset. Linux CI remains useful but cannot substitute for it.

## Reconsideration gates

Open a supported-tier proposal only after:

1. a maintained genuine WSL2 runner is available;
2. the architecture/security spike decides filesystem, Windows-executable, and
   sandbox boundaries;
3. the deterministic PTY and managed-backend subset passes on at least two
   named distributions;
4. installer/Doctor can detect WSL and report exact limitations; and
5. user documentation can state a narrow WSL tier without implying native
   Windows support.

Until those gates pass, the product statement is: **macOS and native Linux are
supported platforms; WSL is unqualified research; native Windows is
unsupported.**
