# MochiPort

Domain language for Codex thread settings exposed through MochiPort's IM
channels.

## Thread Settings

**Thread Model**:
The model selected for subsequent turns in one Codex thread.
_Avoid_: Chat model, session engine

**Reasoning Effort**:
A model-specific Codex thread setting that controls the requested reasoning
depth for subsequent turns.
_Avoid_: Reasoning speed, thinking mode

**Fast Mode**:
A per-thread service-tier setting that requests the Fast tier for a supported
model. It increases model speed at a higher usage rate; Standard is the
non-Fast setting.
_Avoid_: Reasoning effort, a different model

**Settings Draft**:
The Telegram-local, unapplied proposed values for Thread Model, Reasoning
Effort, and Fast Mode in one bound Codex thread.
_Avoid_: Saved settings, global profile
