---
# Markdown projection (§7.2) of the Decision node from northwind.yaml.
# The bundle path derives from the primary term, so this file lands at
# sales/decisions/2026-northwind-renewal.md in an exported bundle.
id: 5c77…
rev: sha256:2b90…
type: corp/Decision@1
classifications: [org/sales, lifecycle/active]
status: approved
decided_on: 2026-08-01
links:
  - { type: decided_by, to: ma41… }
  - { type: decided_in, to: 0t3a… }
---

Renew Northwind at flat ARR and drop the overage clause.

Maria approved after reviewing the [[9a1b|Northwind MSA]]. Full context
is in the [[0t3a|2026-08-01 account review]] transcript. A hand edit to
this file re-ingests as a proposal under the narrow write path (§7.2)
and faces the same admission the original changeset did.
