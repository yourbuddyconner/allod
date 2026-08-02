# supply: cross-organization supply chain

The federation showcase (Part 9). A supply chain is many sovereign
graphs: a supplier's, a manufacturer's, an auditor's, each with its
own root authority, exchanging exactly what a grant permits. Writers
distrust each other here, which is the setting where provenance,
admission, and attestation stop being polish and start being the
product.

## Contents

| File | Contents |
|---|---|
| [ontology.yaml](ontology.yaml) | Parts, batches, facilities, certifications, shipments, inspections |
| [taxonomy.yaml](taxonomy.yaml) | Disclosure regions and compliance regions |
| [policy.yaml](policy.yaml) | Certification review and import discipline |
| [examples/two-graphs.yaml](examples/two-graphs.yaml) | A supplier certifies a batch, a manufacturer imports it, a customer verifies a predicate |

## The federation flow

1. **The supplier's graph** holds facilities, batches, and a
   conflict-free certification anchored to the certificate bytes.
2. **The manufacturer** records the supplier as a peer (§9.3), holds a
   grant scoped to `disclosure/customer`, and imports the
   certification through its own admission flow (§9.6). The imported
   object's lineage points at the supplier's changeset by `allod:`
   reference, so the provenance chain crosses the boundary intact.
3. **The customer** never sees the supplier list. At L3 the
   manufacturer answers with an attested predicate (§5.4): "every
   component batch in this shipment carries an unexpired conflict-free
   certification," proven, with the predicate's code identity in the
   measurement.

## Why the graph shape matters

`component_of` edges form the bill of materials, and certifications
attach to parts, batches, or facilities. The question a compliance
team actually asks, "does every component of this assembly trace to a
certified source," is a walk over BOM edges and `certifies` edges,
where every hop is signed and every certificate resolves to bytes.

Disclosure regions make sharing a classification decision: what a
customer may see is tagged `disclosure/customer`, and the grant scope
is that region. Changing what gets shared is a governed mutation with
a decision record, which is what "we control our data sharing" should
mean.

## Status

Draft, tracks spec v0.3. Import hashes are placeholders until the
reference implementation generates real state hashes (Appendix H).
