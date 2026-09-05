# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

This repository standardizes on **`AGENTS.md`** as the single source of guidance
for coding agents. It covers the commands (build, nightly clippy, tests, index,
eval), the four-crate architecture and data flow, the key design decisions, and
the conventions and gotchas (including the `#[expect]`-over-`#[allow]` lint
rule and the MCP-tools workflow for investigating Rust dependencies).

Read **`AGENTS.md`** before working in this repository.
