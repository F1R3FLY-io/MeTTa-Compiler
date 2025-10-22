# MeTTaTron Packaging Infrastructure

This directory contains packaging configurations for distributing MeTTaTron across multiple platforms and package managers.

## Directory Structure

```
packaging/
├── arch/           # Arch Linux PKGBUILD
├── chocolatey/     # Windows Chocolatey package
├── homebrew/       # macOS Homebrew formula
└── README.md       # This file
```

## Package Formats

### Debian/Ubuntu (.deb)
- Configuration in `Cargo.toml` under `[package.metadata.deb]`
- Built using `cargo-deb`
- Published to APT repository at `f1r3fly-io.github.io/mettatron-apt`

### RedHat/Fedora (.rpm)
- Configuration in `Cargo.toml` under `[package.metadata.generate-rpm]`
- Built using `cargo-generate-rpm`
- Published to YUM repository at `f1r3fly-io.github.io/mettatron-yum`

### Arch Linux
- `arch/PKGBUILD` contains the build recipe
- Users build from source using `makepkg`
- PKGBUILD hosted at `f1r3fly-io.github.io/mettatron-arch`

### macOS (Homebrew)
- `homebrew/mettatron.rb` is the Homebrew formula
- Published to tap at `f1r3fly-io/homebrew-mettatron`
- Supports both x86_64 and ARM64 (Apple Silicon)

### Windows (Chocolatey)
- `chocolatey/mettatron.nuspec` contains package metadata
- `chocolatey/tools/chocolateyinstall.ps1` is the install script
- Published to Chocolatey community repository

## Repository Infrastructure

### GitHub Repositories Required

The following repositories need to be created for hosting packages:

1. **`f1r3fly-io/mettatron-apt`** (APT repository)
   - Branch: `gh-pages`
   - Hosts `.deb` packages and APT metadata
   - Served via GitHub Pages

2. **`f1r3fly-io/mettatron-yum`** (YUM repository)
   - Branch: `gh-pages`
   - Hosts `.rpm` packages and YUM metadata
   - Served via GitHub Pages

3. **`f1r3fly-io/mettatron-arch`** (Arch repository)
   - Branch: `gh-pages`
   - Hosts PKGBUILD file
   - Served via GitHub Pages

4. **`f1r3fly-io/homebrew-mettatron`** (Homebrew tap)
   - Branch: `main`
   - Contains `Formula/mettatron.rb`
   - Standard Homebrew tap structure

## Automation

### Release Workflow (`.github/workflows/release.yml`)

Triggers on:
- Git tags matching `v*.*.*`
- Manual workflow dispatch

Jobs:
1. **build-matrix**: Builds binaries for all platforms
   - Linux x86_64
   - Linux ARM64
   - macOS x86_64 (Intel)
   - macOS ARM64 (Apple Silicon)
   - Windows x86_64

2. **package-deb**: Creates `.deb` package for Debian/Ubuntu

3. **package-rpm**: Creates `.rpm` package for RedHat/Fedora

4. **package-macos-dmg**: Creates `.dmg` installer for macOS

5. **package-windows-installer**: Packages Windows binaries

6. **create-release**: Creates GitHub Release with all artifacts

### Repository Publishing Workflow (`.github/workflows/publish-repos.yml`)

Triggers on:
- GitHub Release published
- Manual workflow dispatch

Jobs:
1. **publish-apt-repo**: Updates APT repository with new `.deb`
2. **publish-yum-repo**: Updates YUM repository with new `.rpm`
3. **publish-pacman-repo**: Updates Arch repository with new PKGBUILD
4. **update-homebrew-tap**: Updates Homebrew formula

## Setting Up Repositories

### 1. Create APT Repository

```bash
# Create repository
gh repo create f1r3fly-io/mettatron-apt --public

# Initialize gh-pages branch
git clone https://github.com/f1r3fly-io/mettatron-apt.git
cd mettatron-apt
git checkout --orphan gh-pages
mkdir -p pool/main/m/mettatron
git add .
git commit -m "Initialize APT repository"
git push origin gh-pages

# Enable GitHub Pages in repository settings
gh api repos/f1r3fly-io/mettatron-apt/pages -X POST -f source='{"branch":"gh-pages","path":"/"}'
```

### 2. Create YUM Repository

```bash
# Create repository
gh repo create f1r3fly-io/mettatron-yum --public

# Initialize gh-pages branch
git clone https://github.com/f1r3fly-io/mettatron-yum.git
cd mettatron-yum
git checkout --orphan gh-pages
mkdir rpms
git add .
git commit -m "Initialize YUM repository"
git push origin gh-pages

# Enable GitHub Pages
gh api repos/f1r3fly-io/mettatron-yum/pages -X POST -f source='{"branch":"gh-pages","path":"/"}'
```

### 3. Create Arch Repository

```bash
# Create repository
gh repo create f1r3fly-io/mettatron-arch --public

# Initialize gh-pages branch
git clone https://github.com/f1r3fly-io/mettatron-arch.git
cd mettatron-arch
git checkout --orphan gh-pages
cp /path/to/MeTTa-Compiler/packaging/arch/PKGBUILD .
git add PKGBUILD
git commit -m "Add PKGBUILD"
git push origin gh-pages

# Enable GitHub Pages
gh api repos/f1r3fly-io/mettatron-arch/pages -X POST -f source='{"branch":"gh-pages","path":"/"}'
```

### 4. Create Homebrew Tap

```bash
# Create repository (must be named homebrew-*)
gh repo create f1r3fly-io/homebrew-mettatron --public

# Initialize repository
git clone https://github.com/f1r3fly-io/homebrew-mettatron.git
cd homebrew-mettatron
mkdir Formula
cp /path/to/MeTTa-Compiler/packaging/homebrew/mettatron.rb Formula/
git add Formula/mettatron.rb
git commit -m "Add mettatron formula"
git push origin main
```

## Manual Package Building

### Building .deb locally

```bash
cargo install cargo-deb
cd MeTTa-Compiler
cargo deb
# Package will be in target/debian/
```

### Building .rpm locally

```bash
cargo install cargo-generate-rpm
cd MeTTa-Compiler
cargo build --release
cargo generate-rpm
# Package will be in target/generate-rpm/
```

### Building for Arch Linux

```bash
cd packaging/arch
makepkg
# Package will be in current directory
```

### Testing Homebrew formula

```bash
brew install --build-from-source packaging/homebrew/mettatron.rb
```

### Testing Chocolatey package

```powershell
cd packaging\chocolatey
choco pack
choco install mettatron -source .
```

## Updating Packages for New Releases

1. Update version in `Cargo.toml`
2. Create and push git tag: `git tag v0.2.0 && git push origin v0.2.0`
3. GitHub Actions will automatically:
   - Build binaries for all platforms
   - Create packages (.deb, .rpm, etc.)
   - Publish GitHub Release
   - Update package repositories

## Troubleshooting

### APT repository not updating
- Check GitHub Pages is enabled
- Verify gh-pages branch exists
- Check workflow logs in Actions tab

### Homebrew formula SHA256 mismatch
- Update SHA256 in `mettatron.rb` after creating release
- Run: `shasum -a 256 mettatron-*.tar.gz`

### Chocolatey package checksum error
- Update checksum in `chocolateyinstall.ps1`
- Run: `checksum -t=sha256 -f=mettatron-windows-x86_64.zip`

## Resources

- [cargo-deb documentation](https://github.com/kornelski/cargo-deb)
- [cargo-generate-rpm documentation](https://github.com/cat-in-136/cargo-generate-rpm)
- [Homebrew formula cookbook](https://docs.brew.sh/Formula-Cookbook)
- [Chocolatey package creation](https://docs.chocolatey.org/en-us/create/create-packages)
- [GitHub Pages documentation](https://docs.github.com/en/pages)
