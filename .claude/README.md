# Claude AI Documentation

This directory contains documentation specifically for Claude Code (claude.ai/code) when working with this codebase.

## Purpose

These files provide context, planning examples, and implementation guides for AI assistants working on this project. They are not intended for end users.

## Directory Structure

```
.claude/
├── README.md            # This file
├── CLAUDE.md            # Main project instructions for Claude Code
├── docs/                # Claude-specific guides and examples
│   ├── DYNAMIC_PLANNING.md
│   ├── MULTISTEP_PLANNING.md
│   ├── EXTRACT_OUTPUTS_GUIDE.md
│   ├── QUICK_REFERENCE_EXTRACTION.md
│   └── PRETTY_PRINTER_METTA_DISPLAY.md
└── archive/             # Historical Claude-specific documents
    └── (18 archived completion summaries and guides)
```

## Files

### Project Instructions
- **CLAUDE.md** - Main instructions for Claude Code about the project architecture, build system, and development workflow

### Claude-Specific Guides (`docs/`)
- **DYNAMIC_PLANNING.md** - Dynamic planning pattern examples
- **MULTISTEP_PLANNING.md** - Multi-step planning examples
- **EXTRACT_OUTPUTS_GUIDE.md** - Guide for extracting outputs from MeTTa evaluation
- **QUICK_REFERENCE_EXTRACTION.md** - Quick reference for common patterns
- **PRETTY_PRINTER_METTA_DISPLAY.md** - Pretty printer implementation

### Historical Documentation (`archive/`)
Contains 18 archived Claude-specific documents including:
- Implementation completion summaries
- Integration milestone achievements
- Threading configuration implementation
- PathMap integration details
- Robot planning summaries

## For Users

If you're a human developer, you probably want the documentation in `docs/` instead. These files are primarily for AI assistants to understand the project structure and implementation patterns.
