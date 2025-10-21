# Installing MeTTaTron

MeTTaTron is available for Linux (x86_64 and ARM64), macOS (x86_64 and ARM64), and Windows (x86_64).

## Table of Contents

- [Ubuntu/Debian (APT)](#ubuntudebian-apt)
- [RedHat/Fedora/CentOS (YUM/DNF)](#redhatfedoracentos-yumdnf)
- [Arch Linux (Pacman/AUR)](#arch-linux-pacmanaur)
- [macOS (Homebrew)](#macos-homebrew)
- [Windows (Chocolatey)](#windows-chocolatey)
- [Generic Installation (All Platforms)](#generic-installation)
- [Building from Source](#building-from-source)

---

## Ubuntu/Debian (APT)

### Add the repository

```bash
# Add repository key
curl -fsSL https://f1r3fly-io.github.io/mettatron-apt/KEY.gpg | sudo gpg --dearmor -o /usr/share/keyrings/mettatron-archive-keyring.gpg

# Add repository to sources
echo "deb [signed-by=/usr/share/keyrings/mettatron-archive-keyring.gpg] https://f1r3fly-io.github.io/mettatron-apt stable main" | sudo tee /etc/apt/sources.list.d/mettatron.list

# Update package list
sudo apt update
```

### Install

```bash
sudo apt install mettatron
```

### Verify installation

```bash
mettatron --version
```

---

## RedHat/Fedora/CentOS (YUM/DNF)

### Add the repository

```bash
# Create repository file
sudo tee /etc/yum.repos.d/mettatron.repo <<EOF
[mettatron]
name=MeTTaTron Repository
baseurl=https://f1r3fly-io.github.io/mettatron-yum
enabled=1
gpgcheck=0
EOF
```

### Install

**For RHEL/CentOS:**
```bash
sudo yum install mettatron
```

**For Fedora:**
```bash
sudo dnf install mettatron
```

### Verify installation

```bash
mettatron --version
```

---

## Arch Linux (Pacman/AUR)

### Option 1: Build from AUR

```bash
# Clone the PKGBUILD
git clone https://github.com/F1R3FLY-io/mettatron-arch.git
cd mettatron-arch

# Build and install
makepkg -si
```

### Option 2: Use an AUR helper (e.g., yay)

```bash
yay -S mettatron
```

### Verify installation

```bash
mettatron --version
```

---

## macOS (Homebrew)

### Add the tap

```bash
brew tap f1r3fly-io/mettatron
```

### Install

```bash
brew install mettatron
```

### Verify installation

```bash
mettatron --version
```

---

## Windows (Chocolatey)

### Install

```powershell
choco install mettatron
```

### Verify installation

```powershell
mettatron --version
```

---

## Generic Installation

### Download prebuilt binaries

Visit the [releases page](https://github.com/F1R3FLY-io/MeTTa-Compiler/releases) and download the appropriate archive for your platform:

- **Linux x86_64**: `mettatron-linux-x86_64.tar.gz`
- **Linux ARM64**: `mettatron-linux-arm64.tar.gz`
- **macOS x86_64**: `mettatron-macos-x86_64.tar.gz`
- **macOS ARM64** (Apple Silicon): `mettatron-macos-arm64.tar.gz`
- **Windows x86_64**: `mettatron-windows-x86_64.zip`

### Extract and install (Unix/Linux/macOS)

```bash
# Extract the archive
tar xzf mettatron-*.tar.gz

# Move binaries to a directory in your PATH
sudo mv mettatron /usr/local/bin/
sudo mv rholang-cli /usr/local/bin/

# Make executable
sudo chmod +x /usr/local/bin/mettatron
sudo chmod +x /usr/local/bin/rholang-cli
```

### Extract and install (Windows)

1. Extract the `.zip` file
2. Add the extracted directory to your PATH:
   - Open "Environment Variables" in System Properties
   - Edit the `Path` variable
   - Add the directory containing `mettatron.exe`
3. Restart your terminal

### Verify installation

```bash
mettatron --version
```

---

## Building from Source

### Prerequisites

- **Rust** (nightly toolchain)
- **Cargo** (comes with Rust)
- **protobuf compiler** (protoc)
- **Git**

### Clone the repositories

```bash
# Create a workspace directory
mkdir mettatron-workspace
cd mettatron-workspace

# Clone MeTTa-Compiler
git clone https://github.com/F1R3FLY-io/MeTTa-Compiler.git

# Clone dependencies
git clone --branch dylon/mettatron https://github.com/F1R3FLY-io/f1r3node.git
git clone --branch main https://github.com/trueagi-io/MORK.git
git clone --branch master https://github.com/Adam-Vandervorst/PathMap.git
```

### Build

```bash
cd MeTTa-Compiler
cargo build --release
```

### Install

```bash
# The binary will be at target/release/mettatron
sudo cp target/release/mettatron /usr/local/bin/

# Optionally install rholang-cli
cd ../f1r3node/rholang
cargo build --release --bin rholang-cli
sudo cp ../../f1r3node/target/release/rholang-cli /usr/local/bin/
```

### Verify installation

```bash
mettatron --version
```

---

## Uninstallation

### Ubuntu/Debian (APT)

```bash
sudo apt remove mettatron
sudo rm /etc/apt/sources.list.d/mettatron.list
```

### RedHat/Fedora/CentOS

```bash
sudo yum remove mettatron  # or: sudo dnf remove mettatron
sudo rm /etc/yum.repos.d/mettatron.repo
```

### Arch Linux

```bash
sudo pacman -R mettatron
```

### macOS (Homebrew)

```bash
brew uninstall mettatron
brew untap f1r3fly-io/mettatron
```

### Windows (Chocolatey)

```powershell
choco uninstall mettatron
```

### Manual installation

```bash
sudo rm /usr/local/bin/mettatron
sudo rm /usr/local/bin/rholang-cli
```

---

## Troubleshooting

### Linux: "protoc: command not found"

Install the protobuf compiler:

**Ubuntu/Debian:**
```bash
sudo apt-get install protobuf-compiler
```

**Fedora:**
```bash
sudo dnf install protobuf-compiler
```

**Arch Linux:**
```bash
sudo pacman -S protobuf
```

### macOS: "protoc: command not found"

```bash
brew install protobuf
```

### Windows: Build errors

Ensure you have:
1. Visual Studio Build Tools installed
2. protoc in your PATH (install via `choco install protoc`)

### CPU feature errors (AES/SSE2 required)

MeTTaTron requires a CPU with AES and SSE2 support. Most CPUs from 2010 onwards have these features.

To check:
- **Linux**: `grep -E 'aes|sse2' /proc/cpuinfo`
- **macOS**: `sysctl machdep.cpu.features`
- **Windows**: Use CPU-Z or similar tool

---

## Getting Help

- **Issues**: https://github.com/F1R3FLY-io/MeTTa-Compiler/issues
- **Documentation**: https://github.com/F1R3FLY-io/MeTTa-Compiler/blob/main/README.md
