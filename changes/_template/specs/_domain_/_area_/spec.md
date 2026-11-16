# Delta: {domain}/{area}

Apply to `spec/{domain}/{area}/spec.md` at archive time. Describe the difference, not the whole file.

## ADDED Requirements

### R-<domain>-<NN>  {name ≤6 words}

WHEN {trigger} THE SYSTEM SHALL {response}.
Acceptance: @R-<domain>-<NN>
Why: {only if an implementer would otherwise violate this: ≤25 words}

### R-<domain>-<NN>  {build fact the lanes cannot observe}

THE SYSTEM SHALL {response}.
Acceptance: gate {name in AGENTS.md}

A ref can also name a review check, when no gate and no lane can observe the rule:
`Acceptance: spec-review check {n}`, numbering a check in `.agents/skills/spec-review/SKILL.md`.
Why: {optional}

## MODIFIED Requirements

### R-<domain>-<NN>  {name}

WHEN {trigger} THE SYSTEM SHALL {new response}.
Acceptance: @R-<domain>-<NN>
Why: {only if an implementer would otherwise violate this: ≤25 words}

## REMOVED Requirements

### R-<domain>-<NN>  {name}

Acceptance: @R-<domain>-<NN>
Supersedes: R-<domain>-<NN> → R-<other>-<MM> in `spec/{domain}/{area}/spec.md`.
