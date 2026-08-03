# Part 10: Non-Goals for v1

These items are out of scope. Each one is deferrable because of a design
decision already made, and the note on each item names that decision.
Declining to specify these is what keeps the core and the federation
layer implementable by one person in finite time.

## 10.1 Global discovery

Graph IDs are self-certifying and peer records carry locator hints
(§9.2, §9.3), which is enough for parties who can exchange an
identifier. Registries, DHTs, and search across unknown graphs are a
service layer that composes on top. The UX of requesting and
negotiating access between strangers also stays out of scope. The grant
object (§9.4) is the interchange point such services would emit.

## 10.2 Query language

No query language is defined. The data model maps cleanly onto existing
ones: property-graph engines through the Parquet binding or direct
mapping, and SPARQL through the Appendix D export. A query standard
before multiple implementations exist would be speculation.

## 10.3 Economic and incentive layers

This spec defines no tokens, marketplaces, or payment rails for
knowledge exchange. Decision records, grants, and attestations give a
future layer the integrity foundation it would need. Building that layer
is future work for another spec.

## 10.4 Key management UX

Custody, recovery flows, and hardware-wallet ergonomics for root
authority keys belong to profiles and products (§6.4). The spec
declares only the rotation and recovery semantics (§4.6, §6.2).

## 10.5 Real-time collaboration

CRDT-style concurrent editing is not a goal. Allod's merge model
(§3.2.3) requires explicit, governed resolution, which rules out
automatic instant convergence. A CRDT layer could emit changesets as
its checkpoint mechanism. The converse, making the log itself a CRDT,
would sacrifice the no-silent-merge property that governance requires.

## 10.6 Being a database

Allod specifies interchange, history, governance, and transfer. Storage
engines, indexes, caches, and query planners are implementation
concerns that conformance does not see.
