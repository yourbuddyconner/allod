#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["pyyaml"]
# ///
"""Lint Allod ontology packages against the specification.

Validates the projection-form YAML under ontologies/ against the rules
of Part 1 (object model), Part 2 (schema), and Part 4 (policy):

  ontology.yaml   type grammar, edge domains and ranges, cardinality,
                  inheritance, import declarations
  taxonomy.yaml   term uniqueness, parent resolution, acyclicity
  policy.yaml     selector grammar (all/any/not), requirement
                  vocabulary, role and region resolution
  examples/*.yaml instance envelopes per object kind, type and term
                  resolution, required attributes, hash prefixes

Usage:
  uv run tools/allod-lint.py [ontologies-dir]

Exit code 1 when any error is found. State hashes in imports are
placeholders until the reference implementation ships Appendix H
vectors, so hash values are checked for algorithm prefixes only.
"""

import re
import sys
from pathlib import Path

import yaml

BASE_TYPES = {
    "string", "int", "float", "decimal", "bool", "timestamp", "date",
    "duration", "bytes", "node-ref", "edge-ref", "document-ref",
    "external-ref",
}
CARDINALITIES = {"one-to-one", "one-to-many", "many-to-one", "many-to-many"}
POSTURES = {"open", "restricted"}
SELECTOR_KEYS = {
    "author_kind", "basis", "region", "type", "operation",
    "substrate", "repo", "target_ref",
}
AUTHOR_KINDS = {"user", "service", "agent"}
BASES = {"manual", "deterministic", "model-assisted"}
REQUIREMENT_KEYS = {
    "authors", "reviewers", "review_window", "schema_valid",
    "classification_required", "attestation_required", "substrate_checks",
}
OPERATIONS = {
    "create", "update", "delete", "resolve", "redact-document",
    "redact-operation", "define-type", "set-policy", "deprecate-term",
}
DOC_KINDS = {"node", "edge", "classification", "document", "changeset",
             "decision-record"}
VERDICTS = {"approve", "reject", "abstain"}
STORAGE = {"inline", "stored", "external"}
TERM_STATUS = {"active", "deprecated"}
HASH_RE = re.compile(r"^[a-z0-9-]+:")
HASH_FIELDS = {"rev", "hash", "content_hash", "schema_context",
               "state_hash", "policy_context"}


class Linter:
    def __init__(self):
        self.errors = []
        self.warnings = []
        self.packages = {}      # name -> {"types": {...}, "edges": {...}, "path": Path}
        self.taxonomies = {}    # name -> {"terms": {name: parents}, "path": Path}
        self.pending = []       # deferred callables run after all files load

    def error(self, path, where, msg):
        self.errors.append(f"{path}: {where}: {msg}")

    def warn(self, path, where, msg):
        self.warnings.append(f"{path}: {where}: {msg}")

    # ---------------- loading ----------------

    def load_dir(self, root: Path):
        docs = []
        for path in sorted(root.rglob("*.yaml")):
            try:
                loaded = list(yaml.safe_load_all(path.read_text()))
            except yaml.YAMLError as exc:
                self.error(path, "yaml", f"parse failure: {exc}")
                continue
            for doc in loaded:
                if isinstance(doc, dict):
                    docs.append((path, doc))
        # Register schemas first so references resolve regardless of order.
        for path, doc in docs:
            if "ontology" in doc:
                self.register_ontology(path, doc)
            elif "taxonomy" in doc:
                self.register_taxonomy(path, doc)
        for path, doc in docs:
            if "ontology" in doc:
                self.pending.append(lambda p=path, d=doc: self.check_ontology(p, d))
            elif "taxonomy" in doc:
                self.pending.append(lambda p=path, d=doc: self.check_taxonomy(p, d))
            elif "policy" in doc:
                self.pending.append(lambda p=path, d=doc: self.check_policy(p, d))
            elif "kind" in doc:
                self.pending.append(lambda p=path, d=doc: self.check_instance(p, d))
            else:
                self.warn(path, "document", "unrecognized document shape")

    def register_ontology(self, path, doc):
        name = doc.get("ontology")
        if not isinstance(name, str):
            self.error(path, "ontology", "missing or invalid package name")
            return
        self.packages[name] = {
            "types": doc.get("entity_types") or {},
            "edges": doc.get("edge_types") or {},
            "imports": [i.get("ontology") for i in doc.get("imports") or []
                        if isinstance(i, dict)],
            "path": path,
        }

    def register_taxonomy(self, path, doc):
        name = doc.get("taxonomy")
        if not isinstance(name, str):
            self.error(path, "taxonomy", "missing or invalid taxonomy name")
            return
        terms = {}
        for term in doc.get("terms") or []:
            if isinstance(term, dict) and isinstance(term.get("name"), str):
                terms[term["name"]] = term.get("parents") or []
        self.taxonomies[name] = {"terms": terms, "path": path}

    # ---------------- resolution helpers ----------------

    def resolve_type(self, ref, pkg=None):
        """Resolve 'Type' or 'pkg/Type', returning (pkg, name, def) or None."""
        if not isinstance(ref, str):
            return None
        bare = ref.split("@")[0]
        if "/" in bare:
            pname, tname = bare.rsplit("/", 1)
            info = self.packages.get(pname)
            if info and tname in info["types"]:
                return (pname, tname, info["types"][tname])
            return None
        if pkg:
            info = self.packages.get(pkg)
            if info and bare in info["types"]:
                return (pkg, bare, info["types"][bare])
        return None

    def resolve_edge_type(self, ref):
        bare = ref.split("@")[0]
        if "/" not in bare:
            return None
        pname, ename = bare.rsplit("/", 1)
        info = self.packages.get(pname)
        if info and ename in info["edges"]:
            return (pname, ename, info["edges"][ename])
        return None

    def term_exists(self, ref):
        bare = ref.split("@")[0]
        return any(bare in t["terms"] for t in self.taxonomies.values())

    def collected_attrs(self, pkg, tname, seen=None):
        """Attribute map including inherited attributes."""
        seen = seen or set()
        if (pkg, tname) in seen:
            return {}
        seen.add((pkg, tname))
        info = self.packages.get(pkg)
        if not info or tname not in info["types"]:
            return {}
        tdef = info["types"][tname] or {}
        attrs = dict(tdef.get("attributes") or {})
        parent = tdef.get("extends")
        if parent:
            resolved = self.resolve_type(parent, pkg)
            if resolved:
                inherited = self.collected_attrs(resolved[0], resolved[1], seen)
                for aname, adef in inherited.items():
                    attrs.setdefault(aname, adef)
        return attrs

    # ---------------- type expression grammar ----------------

    def check_type_expr(self, path, where, expr):
        if not isinstance(expr, str):
            self.error(path, where, f"attribute type must be a string, got {expr!r}")
            return
        expr = expr.strip()
        if expr in BASE_TYPES:
            return
        if expr.startswith("list<") and expr.endswith(">"):
            self.check_type_expr(path, where, expr[5:-1])
            return
        if expr.startswith("map<") and expr.endswith(">"):
            inner = expr[4:-1]
            depth, split = 0, -1
            for i, ch in enumerate(inner):
                if ch == "<":
                    depth += 1
                elif ch == ">":
                    depth -= 1
                elif ch == "," and depth == 0:
                    split = i
                    break
            if split < 0:
                self.error(path, where, f"map type needs key and value: {expr}")
                return
            key = inner[:split].strip()
            if key != "string":
                self.error(path, where, f"map keys must be string, got {key}")
            self.check_type_expr(path, where, inner[split + 1:].strip())
            return
        self.error(path, where, f"unknown attribute type {expr!r}")

    # ---------------- schema checks ----------------

    def check_ontology(self, path, doc):
        pkg = doc.get("ontology")
        where = f"ontology {pkg}"
        version = doc.get("version")
        if not isinstance(version, int) or version < 1:
            self.error(path, where, "version must be an integer >= 1")
        for imp in doc.get("imports") or []:
            if not isinstance(imp, dict) or "ontology" not in imp:
                self.error(path, where, f"malformed import {imp!r}")
                continue
            if imp["ontology"] not in self.packages:
                self.error(path, where,
                           f"import {imp['ontology']!r} is not a loaded package")
            sh = imp.get("state_hash", "")
            if not isinstance(sh, str) or not HASH_RE.match(sh):
                self.error(path, where,
                           f"import {imp['ontology']!r} state_hash lacks an "
                           "algorithm prefix (§1.7)")
        imports = set(self.packages.get(pkg, {}).get("imports", []))

        def foreign_ok(ref):
            bare = ref.split("@")[0]
            if "/" not in bare:
                return True
            target_pkg = bare.rsplit("/", 1)[0]
            return target_pkg == pkg or target_pkg in imports

        for tname, tdef in (doc.get("entity_types") or {}).items():
            twhere = f"entity_types.{tname}"
            tdef = tdef or {}
            for aname, adef in (tdef.get("attributes") or {}).items():
                awhere = f"{twhere}.attributes.{aname}"
                if not isinstance(adef, dict) or "type" not in adef:
                    self.error(path, awhere, "attribute needs a type")
                    continue
                self.check_type_expr(path, awhere, adef["type"])
                req = adef.get("required")
                if req is not None and not isinstance(req, bool):
                    self.error(path, awhere, "required must be a bool")
                target = adef.get("target")
                if target is not None:
                    if adef["type"] != "node-ref":
                        self.error(path, awhere, "target is only valid on node-ref")
                    elif not self.resolve_type(target, pkg):
                        self.error(path, awhere, f"target {target!r} does not resolve")
            parent = tdef.get("extends")
            if parent:
                resolved = self.resolve_type(parent, pkg)
                if not resolved:
                    self.error(path, twhere, f"extends {parent!r} does not resolve")
                elif not foreign_ok(parent):
                    self.error(path, twhere,
                               f"extends {parent!r} but package is not imported")
                else:
                    inherited = self.collected_attrs(resolved[0], resolved[1])
                    for aname, adef in (tdef.get("attributes") or {}).items():
                        if aname in inherited and isinstance(adef, dict):
                            if adef.get("type") != inherited[aname].get("type"):
                                self.error(path, f"{twhere}.attributes.{aname}",
                                           "redefines an inherited attribute with a "
                                           "different type (§2.3 forbids removal or "
                                           "widening)")

        for ename, edef in (doc.get("edge_types") or {}).items():
            ewhere = f"edge_types.{ename}"
            edef = edef or {}
            card = edef.get("cardinality")
            if card not in CARDINALITIES:
                self.error(path, ewhere,
                           f"cardinality {card!r} not in {sorted(CARDINALITIES)} (§2.1)")
            for side in ("domain", "range"):
                val = edef.get(side)
                if val is None:
                    self.error(path, ewhere, f"missing {side}")
                    continue
                refs = val if isinstance(val, list) else [val]
                for ref in refs:
                    if not self.resolve_type(ref, pkg):
                        self.error(path, ewhere, f"{side} {ref!r} does not resolve")
                    elif not foreign_ok(ref):
                        self.error(path, ewhere,
                                   f"{side} {ref!r} but package is not imported")
            for aname, adef in (edef.get("attributes") or {}).items():
                if not isinstance(adef, dict) or "type" not in adef:
                    self.error(path, f"{ewhere}.attributes.{aname}",
                               "attribute needs a type")
                else:
                    self.check_type_expr(path, f"{ewhere}.attributes.{aname}",
                                         adef["type"])

        for rule in doc.get("validation_rules") or []:
            if not isinstance(rule, dict) or "name" not in rule or "rule" not in rule:
                self.error(path, "validation_rules",
                           f"rules need name and rule: {rule!r}")

    def check_taxonomy(self, path, doc):
        name = doc.get("taxonomy")
        where = f"taxonomy {name}"
        version = doc.get("version")
        if not isinstance(version, int) or version < 1:
            self.error(path, where, "version must be an integer >= 1")
        seen = set()
        terms = doc.get("terms") or []
        for term in terms:
            if not isinstance(term, dict) or "name" not in term:
                self.error(path, where, f"malformed term {term!r}")
                continue
            tname = term["name"]
            if tname in seen:
                self.error(path, where, f"duplicate term {tname!r}")
            seen.add(tname)
            status = term.get("status")
            if status is not None and status not in TERM_STATUS:
                self.error(path, f"{where}.{tname}",
                           f"status {status!r} not in {sorted(TERM_STATUS)}")
            for parent in term.get("parents") or []:
                if parent not in seen and not self.term_exists(parent):
                    self.error(path, f"{where}.{tname}",
                               f"parent {parent!r} does not resolve")
        # Acyclicity (§2.2): DFS over this taxonomy's parent links.
        local = {t["name"]: (t.get("parents") or []) for t in terms
                 if isinstance(t, dict) and "name" in t}
        WHITE, GRAY, BLACK = 0, 1, 2
        color = {t: WHITE for t in local}

        def visit(node, stack):
            color[node] = GRAY
            for parent in local.get(node, []):
                if parent not in local:
                    continue
                if color[parent] == GRAY:
                    self.error(path, where,
                               f"cycle through {' -> '.join(stack + [node, parent])} "
                               "(the structure must be a DAG, §2.2)")
                elif color[parent] == WHITE:
                    visit(parent, stack + [node])
            color[node] = BLACK

        for t in local:
            if color[t] == WHITE:
                visit(t, [])

    # ---------------- policy checks ----------------

    def check_selector(self, path, where, sel, roles):
        if not isinstance(sel, dict):
            self.error(path, where, f"selector must be a map, got {sel!r}")
            return
        for combinator in ("all", "any"):
            if combinator in sel:
                for i, sub in enumerate(sel[combinator] or []):
                    self.check_selector(path, f"{where}.{combinator}[{i}]", sub, roles)
                if set(sel) - {combinator}:
                    self.error(path, where,
                               f"{combinator} does not mix with other keys")
                return
        if "not" in sel:
            self.check_selector(path, f"{where}.not", sel["not"], roles)
            if set(sel) - {"not"}:
                self.error(path, where, "not does not mix with other keys")
            return
        unknown = set(sel) - SELECTOR_KEYS
        if unknown:
            self.error(path, where, f"unknown selector keys {sorted(unknown)} (§4.1)")
        if "author_kind" in sel and sel["author_kind"] not in AUTHOR_KINDS:
            self.error(path, where, f"author_kind {sel['author_kind']!r} "
                                    f"not in {sorted(AUTHOR_KINDS)}")
        if "basis" in sel and sel["basis"] not in BASES:
            self.error(path, where, f"basis {sel['basis']!r} not in {sorted(BASES)}")
        if "region" in sel and not self.term_exists(sel["region"]):
            self.error(path, where,
                       f"region {sel['region']!r} resolves to no taxonomy term")
        if "type" in sel and not self.resolve_type(sel["type"]):
            self.error(path, where, f"type {sel['type']!r} does not resolve")
        if "operation" in sel:
            ops = sel["operation"]
            ops = ops if isinstance(ops, list) else [ops]
            for op in ops:
                if op not in OPERATIONS:
                    self.error(path, where, f"unknown operation {op!r}")

    def check_reviewers(self, path, where, val, roles):
        entries = val if isinstance(val, list) else [val]
        for entry in entries:
            if not isinstance(entry, dict) or "role" not in entry:
                self.error(path, where, f"reviewers entries need a role: {entry!r}")
                continue
            if entry["role"] not in roles:
                self.error(path, where,
                           f"role {entry['role']!r} is not declared in roles")
            quorum = entry.get("quorum")
            if quorum is not None and (not isinstance(quorum, int) or quorum < 1):
                self.error(path, where, "quorum must be an integer >= 1")

    def check_policy(self, path, doc):
        name = doc.get("policy")
        where = f"policy {name}"
        version = doc.get("version")
        if not isinstance(version, int) or version < 1:
            self.error(path, where, "version must be an integer >= 1")
        posture = doc.get("default_posture")
        if posture not in POSTURES:
            self.error(path, where,
                       f"default_posture {posture!r} not in {sorted(POSTURES)} (§4.1)")
        roles = doc.get("roles") or {}
        seen_rules = set()
        for rule in doc.get("rules") or []:
            if not isinstance(rule, dict):
                self.error(path, where, f"malformed rule {rule!r}")
                continue
            rname = rule.get("name", "<unnamed>")
            rwhere = f"{where}.rules.{rname}"
            if rname in seen_rules:
                self.error(path, rwhere, "duplicate rule name")
            seen_rules.add(rname)
            if "select" not in rule or "require" not in rule:
                self.error(path, rwhere, "rules need select and require")
                continue
            self.check_selector(path, f"{rwhere}.select", rule["select"], roles)
            require = rule["require"]
            if not isinstance(require, dict):
                self.error(path, rwhere, "require must be a map")
                continue
            unknown = set(require) - REQUIREMENT_KEYS
            if unknown:
                self.error(path, rwhere,
                           f"unknown requirement keys {sorted(unknown)} (§4.2)")
            if "reviewers" in require:
                self.check_reviewers(path, f"{rwhere}.require.reviewers",
                                     require["reviewers"], roles)
            window = require.get("review_window")
            if window is not None:
                if not isinstance(window, dict) or \
                        set(window) - {"min", "max", "on_expiry"}:
                    self.error(path, rwhere, f"malformed review_window {window!r}")
                elif window.get("on_expiry") not in (None, "reject"):
                    self.error(path, rwhere,
                               f"on_expiry {window['on_expiry']!r} unsupported")

    # ---------------- instance checks ----------------

    def check_hashes(self, path, where, doc):
        for field in HASH_FIELDS & set(doc):
            val = doc[field]
            if isinstance(val, str) and not HASH_RE.match(val):
                self.error(path, where,
                           f"{field} {val!r} lacks an algorithm prefix (§1.7)")

    def check_node_payload(self, path, where, doc):
        ref = doc.get("type")
        resolved = self.resolve_type(ref) if ref else None
        if not resolved:
            self.error(path, where, f"node type {ref!r} does not resolve")
            return
        if "@" not in ref:
            self.error(path, where, f"type ref {ref!r} lacks a version (§1.2)")
        pkg, tname, _ = resolved
        attrs = self.collected_attrs(pkg, tname)
        given = doc.get("attributes") or {}
        for aname, adef in attrs.items():
            if isinstance(adef, dict) and adef.get("required") and aname not in given:
                self.error(path, where,
                           f"missing required attribute {aname!r} of {pkg}/{tname}")
        for aname in given:
            if aname not in attrs:
                self.error(path, where,
                           f"attribute {aname!r} is not declared on {pkg}/{tname}")

    def check_edge_payload(self, path, where, doc):
        ref = doc.get("type")
        resolved = self.resolve_edge_type(ref) if ref else None
        if not resolved:
            self.error(path, where, f"edge type {ref!r} does not resolve")
            return
        if "@" not in ref:
            self.error(path, where, f"type ref {ref!r} lacks a version (§1.4)")
        for side in ("from", "to"):
            if side not in doc:
                self.error(path, where, f"edge missing {side}")
        declared = resolved[2].get("attributes") or {}
        for aname in doc.get("attributes") or {}:
            if aname not in declared:
                self.error(path, where,
                           f"attribute {aname!r} is not declared on edge type {ref}")

    def check_classification_payload(self, path, where, doc, in_op=False):
        for field in ("subject", "term"):
            if field not in doc:
                self.error(path, where, f"classification missing {field}")
        term = doc.get("term")
        if term and not self.term_exists(term):
            self.error(path, where, f"term {term!r} resolves to no taxonomy term")
        if not in_op:
            if "asserted_by" not in doc:
                self.error(path, where, "classification missing asserted_by (§1.6)")
            basis = doc.get("basis")
            if basis not in BASES:
                self.error(path, where,
                           f"basis {basis!r} not in {sorted(BASES)} (§1.6)")

    def check_provenance(self, path, where, prov):
        if not isinstance(prov, dict):
            self.error(path, where, "provenance must be a map (§5.1)")
            return
        method = prov.get("method")
        if method not in BASES:
            self.error(path, where, f"lineage method {method!r} not in {sorted(BASES)}")
        if method in ("deterministic", "model-assisted") and not prov.get("tool"):
            self.error(path, where, "non-manual lineage must name a tool (§5.1)")

    def check_instance(self, path, doc):
        kind = doc.get("kind")
        ident = doc.get("id") or doc.get("hash") or doc.get("subject") or "?"
        where = f"{kind} {str(ident)[:12]}"
        if kind not in DOC_KINDS:
            self.error(path, where, f"unknown kind {kind!r}")
            return
        self.check_hashes(path, where, doc)
        if "provenance" in doc:
            self.check_provenance(path, f"{where}.provenance", doc["provenance"])
        if kind == "node":
            self.check_node_payload(path, where, doc)
        elif kind == "edge":
            self.check_edge_payload(path, where, doc)
        elif kind == "classification":
            self.check_classification_payload(path, where, doc)
        elif kind == "document":
            for field in ("content_hash", "media_type", "storage"):
                if field not in doc:
                    self.error(path, where, f"document missing {field} (§1.5)")
            if doc.get("storage") not in STORAGE:
                self.error(path, where, f"storage {doc.get('storage')!r} "
                                        f"not in {sorted(STORAGE)}")
        elif kind == "changeset":
            for field in ("hash", "parents", "author", "timestamp",
                          "operations", "schema_context", "signature"):
                if field not in doc:
                    self.error(path, where, f"changeset missing {field} (§3.2.1)")
            for i, op in enumerate(doc.get("operations") or []):
                self.check_operation(path, f"{where}.operations[{i}]", op)
        elif kind == "decision-record":
            for field in ("subject", "policy_context", "verdict",
                          "deciders", "timestamp"):
                if field not in doc:
                    self.error(path, where, f"decision record missing {field} (§4.3)")
            if doc.get("verdict") not in VERDICTS:
                self.error(path, where, f"verdict {doc.get('verdict')!r} "
                                        f"not in {sorted(VERDICTS)}")

    def check_operation(self, path, where, op):
        if not isinstance(op, dict) or len(op) != 1:
            self.error(path, where, f"operations carry exactly one verb: {op!r}")
            return
        verb, payload = next(iter(op.items()))
        if verb not in OPERATIONS:
            self.error(path, where, f"unknown operation {verb!r}")
            return
        if not isinstance(payload, dict):
            self.error(path, where, "operation payload must be a map")
            return
        pkind = payload.get("kind")
        if verb == "create":
            if pkind == "node":
                self.check_node_payload(path, where, payload)
            elif pkind == "edge":
                self.check_edge_payload(path, where, payload)
            elif pkind == "classification":
                self.check_classification_payload(path, where, payload, in_op=True)

    # ---------------- driver ----------------

    def run(self, root: Path) -> int:
        self.load_dir(root)
        for check in self.pending:
            check()
        for line in self.errors:
            print(f"ERROR {line}")
        for line in self.warnings:
            print(f"WARN  {line}")
        packages = len(self.packages)
        taxonomies = len(self.taxonomies)
        print(f"\nchecked {packages} packages, {taxonomies} taxonomies: "
              f"{len(self.errors)} errors, {len(self.warnings)} warnings")
        return 1 if self.errors else 0


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("ontologies")
    if not root.is_dir():
        print(f"no such directory: {root}", file=sys.stderr)
        return 2
    return Linter().run(root)


if __name__ == "__main__":
    sys.exit(main())
