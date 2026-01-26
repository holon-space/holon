# Codev Project Instructions for AI Agents

> **Note**: This file follows the [AGENTS.md standard](https://agents.md/) for cross-tool compatibility with Cursor, GitHub Copilot, and other AI coding assistants. For Claude Code users, an identical [CLAUDE.md](CLAUDE.md) file is maintained for native support. Both files contain the same content and should be kept synchronized.

## Project Context

This project uses the Codev context-driven development methodology for structured feature development.

## Active Protocol

**Protocol**: SPIDER
**Location**: `codev/protocols/spider/protocol.md`

This project uses SPIDER with Zen MCP multi-agent consultation for enhanced code review and validation.

## Quick Start

When building new features:

1. **Start with Specification**: Ask clarifying questions, then create a spec document
2. **Create Plan**: Break down the specification into executable phases
3. **Implement in Phases**: Follow the IDE loop (Implement → Defend → Evaluate)
4. **Review**: Document lessons learned

## Directory Structure

```
project-root/
├── codev/
│   ├── protocols/
│   │   ├── spider/             # Full SPIDER protocol with multi-agent consultation
│   │   ├── spider-solo/        # Single-agent variant (fallback)
│   │   └── tick/               # Fast autonomous protocol
│   ├── specs/                  # Feature specifications (WHAT to build)
│   ├── plans/                  # Implementation plans (HOW to build)
│   ├── reviews/                # Reviews and lessons learned
│   ├── resources/              # Reference materials
│   └── agents/                 # AI agent definitions
├── .claude/agents/             # Claude Code agent location
├── AGENTS.md                   # This file
├── CLAUDE.md                   # Claude Code-specific (identical to this file)
└── [project code]
```

## Core Workflow

### For New Features

1. **Specify Phase**:
   - Ask clarifying questions to understand requirements
   - Create specification in `codev/specs/####-descriptive-name.md`
   - Consult other AI agents (GPT-5, Gemini) for feedback
   - Iterate with user feedback
   - Commit iterations with descriptive messages

2. **Plan Phase**:
   - Create implementation plan in `codev/plans/####-descriptive-name.md`
   - Break down into logical phases
   - Consult agents before presenting to user
   - NO time estimates (focus on done/not done)

3. **Implementation Phases** (IDE Loop):
   - **Implement**: Write code for current phase
   - **Defend**: Write comprehensive tests
   - **Evaluate**: Consult agents, get user approval, commit
   - Repeat for each phase

4. **Review Phase**:
   - Create lessons learned document in `codev/reviews/####-descriptive-name.md`
   - Document what went well and what could improve
   - Update project resources if needed

### File Naming Convention

Each feature gets exactly THREE documents with the same base filename:
- `codev/specs/0001-feature-name.md` - The specification
- `codev/plans/0001-feature-name.md` - The implementation plan
- `codev/reviews/0001-feature-name.md` - Review and lessons learned

Use sequential numbering (0001, 0002, etc.) and descriptive names.

## When to Use SPIDER

### Use SPIDER for:
- New feature development
- Architecture changes
- Complex refactoring
- System design decisions
- API design and implementation
- Performance optimization initiatives

### Skip SPIDER for:
- Simple bug fixes (< 10 lines)
- Documentation updates
- Configuration changes
- Dependency updates
- Emergency hotfixes

## Key Principles

1. **Multi-Agent Consultation**: Use Zen MCP to consult GPT-5, Gemini, and other models at key checkpoints
2. **Human Approval**: Always get user approval before proceeding to next phase
3. **Three Documents**: Maintain spec, plan, and review for each feature
4. **Iterative**: Expect multiple rounds of feedback and refinement
5. **Commit Often**: Use git commits to track evolution of documents

## Protocol Details

For complete protocol documentation, see: `codev/protocols/spider/protocol.md`
