# Part 9: Non-Goals for v1

These items are out of scope. Each one is deferrable because of a design
decision already made, and the note on each item names that decision.
Declining to specify these is what keeps L0 through L2 implementable by
one person in finite time.

## 9.1 Federation transport, discovery, and sync

How graphs find each other, negotiate access, and synchronize is not
specified. This can wait because sharing knowledge means shipping
changesets, or a checkpoint plus changesets (Part 3), and verification
never requires the host (§5.3). The unit of exchange is fully specified,
and only the transport is left open. Git followed the same adoption path:
object model first, forges later. A future federation spec composes on
top without changes to the core.

## 9.2 Sharing mechanics and access negotiation

The selective-disclosure primitives are in scope (§5.4). The protocols
and UX for requesting, granting, and revoking access between parties are
not.

## 9.3 Query language

No query language is defined. The data model maps cleanly onto existing
ones: property-graph engines through the Parquet binding or direct
mapping, and SPARQL through the Appendix D export. A query standard
before multiple implementations exist would be speculation.

## 9.4 Economic and incentive layers

This spec defines no tokens, marketplaces, or payment rails for
knowledge exchange. Decision records and attestations give a future
layer the integrity
substrate it would need. Building that layer is future work for another
spec.

## 9.5 Key management UX

Custody, recovery flows, and hardware-wallet ergonomics for root
authority keys belong to profiles and products (§6.4).
The spec declares only the rotation and recovery semantics (§4.6, §6.2).

## 9.6 Real-time collaboration

CRDT-style concurrent editing is not a goal. Allod's merge model
(§3.2.3) requires explicit, governed resolution, which rules out
automatic instant convergence. A CRDT
layer could emit changesets as its checkpoint mechanism. The converse,
making the log itself a CRDT, would sacrifice the no-silent-merge
property that governance requires.

## 9.7 Being a database

Allod specifies interchange, history, and governance. Storage engines,
indexes, caches, and query planners are implementation concerns that
conformance does not see.
