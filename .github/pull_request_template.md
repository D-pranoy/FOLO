<!--
The PR title becomes the squash commit message, so it follows Conventional Commits:
feat:, fix:, docs:, refactor:, test:, build:, ci: or chore:, with ! for a breaking change.
See docs/process/workflow.md.
-->

## What and why

<!-- What changes, and why. Reference the run book step, or state the reason for the change. -->

Run book step: <!-- RB-NN step N, or "none" -->

## Documentation impact

<!-- docs/ is the source of truth: documents change first, in this PR or an earlier one. Tick one. -->

- [ ] Documents updated: <!-- list them -->
- [ ] ADR added or superseded: <!-- ADR-NNNN; needed when a decision changes or a contract breaks -->
- [ ] None, because: <!-- explain -->

## Testing

<!-- How this was verified. If golden files were regenerated, say why the behaviour was meant to change. -->

## Checklist

- [ ] The run book step is referenced, or the reason for the change is stated.
- [ ] Tests are added or updated; golden files are regenerated only with a stated reason.
- [ ] Contracts and generated code are in sync.
- [ ] No secrets, personal data or device addresses in code, tests or docs.
- [ ] Device-facing changes followed the [testing policy](https://github.com/D-pranoy/FOLO/blob/main/docs/process/testing-policy.md): fakes first, live tests announced, at low brightness, devices released afterwards.
