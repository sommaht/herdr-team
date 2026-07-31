#!/bin/sh
# rust-style mechanical gate. Rule definitions: ../references/gate.md
#
# Usage: check.sh [file-or-dir ...]     (default: ./src)
# Exit:  0 = no findings, 1 = findings, 2 = usage error.
#
# POSIX sh + BSD awk/sed/grep compatible (macOS). Heuristic by design: it flags
# candidates with rule IDs; the judgment tier decides edge cases. Known limits
# are printed in the coverage trailer.

# Function-length threshold: the style guides say most functions fit in 10-50
# lines; >50 is flagged as a candidate, not auto-failed (RS-031 is judgment).
FN_FLAG_LINES=50
# File-size prompt: guides call 150-350 comfortable; 400 leaves headroom so
# only clear outliers surface (RS-033 is a prompt to look for boundaries).
FILE_FLAG_LINES=400
# Duplicate-body detection ignores trivial bodies (getters, one-liners, and
# delegations to a shared owner). The bar is statement count, not width: a body
# of one statement is not copy-paste risk however wide that statement is — it is
# an idiom or a call to the one owner the rule asks for, and a char threshold
# only invites answering the finding by rewrapping a line. Counted in normalized
# body lines, which include the closing brace, so 8 means seven statements: the
# rule is aimed at a copied handler, not at two functions that happen to agree.
MIN_DUP_BODY_LINES=8

TMP=$(mktemp -d "${TMPDIR:-/tmp}/rust-style-gate.XXXXXX") || exit 2
trap 'rm -rf "$TMP"' EXIT

# --dump-fns: print the parsed function table (self-test hook).
DUMP_FNS=0
if [ "${1:-}" = "--dump-fns" ]; then DUMP_FNS=1; shift; fi

# ---------------------------------------------------------------------------
# Collect .rs files
# ---------------------------------------------------------------------------
if [ "$#" -eq 0 ]; then
    set -- ./src
fi
: > "$TMP/files"
for target in "$@"; do
    if [ -d "$target" ]; then
        find "$target" -name '*.rs' -not -path '*/target/*' >> "$TMP/files"
    elif [ -f "$target" ]; then
        printf '%s\n' "$target" >> "$TMP/files"
    else
        echo "check.sh: no such file or directory: $target" >&2
        exit 2
    fi
done
sort -u "$TMP/files" -o "$TMP/files"
if ! [ -s "$TMP/files" ]; then
    echo "check.sh: no .rs files found" >&2
    exit 2
fi

# Non-test files: test dirs and tests.rs are exempt from several rules.
grep -vE '(^|/)tests?/|/benches/|(^|/)tests\.rs$' "$TMP/files" > "$TMP/nontest" || true

: > "$TMP/findings"
: > "$TMP/prompts"
# Two output tiers. A finding is a mechanical VIOLATION — the text proves the
# predicate — and sets the exit status. A prompt is a CANDIDATE: the mechanics
# surfaced a shape whose rule has a justified-exception branch (a site comment,
# a doc justification, an in-progress phase) that only the judgment tier can
# verify — so the reviewer decides, and prompts never fail the run.
finding() { printf '%s\n' "$*" >> "$TMP/findings"; }
prompt() { printf '%s\n' "$*" >> "$TMP/prompts"; }

# For rules that exempt inline test modules: strip everything from the first
# `#[cfg(test)]` to EOF (tests conventionally sit at the bottom of the file).
# Output lines keep their original numbers as "N:line".
nontest_body() {
    awk '/^[[:space:]]*#\[cfg\(test\)\]/ { exit } { printf "%d:%s\n", NR, $0 }' "$1"
}

# ---------------------------------------------------------------------------
# Line-pattern rules
# ---------------------------------------------------------------------------

# RS-001: identical map_err closure appearing 2+ times across the input set.
while read -r file; do
    sed -n 's/.*\(\.map_err(.*\)/\1/p' "$file" | tr -d ' \t' | sed 's/[;?,]*$//' \
        | awk -v f="$file" '{ printf "%s\t%s\n", $0, f }'
done < "$TMP/nontest" > "$TMP/maperr"
if [ -s "$TMP/maperr" ]; then
    cut -f1 "$TMP/maperr" | sort | uniq -c | awk '$1 >= 2 { $1=""; sub(/^ /,""); print }' \
    | while read -r closure; do
        count=$(grep -cF "$closure	" "$TMP/maperr" 2>/dev/null || echo 2)
        first=$(grep -F "$closure	" "$TMP/maperr" | head -1 | cut -f2)
        finding "RS-001 $first — identical closure x$count: \`$closure\` — replace with one From impl and \`?\`"
    done
fi

# RS-002 (candidate): map_err(|_| ...) — allowed when the replacement captures
# strictly more data AND a site comment says so; the script cannot see either.
while read -r file; do
    nontest_body "$file" | grep -E 'map_err\(\|_\|' \
    | while IFS=: read -r line _rest; do
        prompt "RS-002 $file:$line — candidate: map_err(|_| ...) discards the source; verify the strictly-more-data + site-comment branch"
    done
done < "$TMP/nontest"

# RS-003: stringified typed errors.
while read -r file; do
    nontest_body "$file" | grep -E 'Result<[^>]*,[[:space:]]*String[[:space:]]*>' \
    | while IFS=: read -r line _rest; do
        finding "RS-003 $file:$line — Result<_, String> boundary; keep the typed error"
    done
    nontest_body "$file" | grep -E 'map_err\([^)]*to_string\(\)' \
    | while IFS=: read -r line _rest; do
        finding "RS-003 $file:$line — error stringified while wrapping"
    done
    nontest_body "$file" | grep -E 'map_err\([^)]*format!' \
    | while IFS=: read -r line _rest; do
        finding "RS-003 $file:$line — error stringified while wrapping (format!)"
    done
done < "$TMP/nontest"

# RS-010 (candidate): fn try_new — multi-arg with a doc justification is exempt
# per gate.md; the script cannot count arguments or read the doc comment.
while read -r file; do
    nontest_body "$file" | grep -E '(pub )?fn try_new' \
    | while IFS=: read -r line _rest; do
        prompt "RS-010 $file:$line — candidate: fn try_new — single-arg validation is TryFrom/FromStr; verify the multi-arg + doc-comment exemption"
    done
done < "$TMP/nontest"

# RS-012 (candidate, report-only): integer `as` casts, per site.
while read -r file; do
    nontest_body "$file" | grep -E '[a-z0-9_)] as (u|i)(8|16|32|64|128|size)' \
    | while IFS=: read -r line _rest; do
        prompt "RS-012 $file:$line — integer \`as\` cast; prefer From/TryFrom outside numeric hot paths"
    done
done < "$TMP/nontest"

# RS-020 (candidate): a hand-rolled serde impl with an adjacent stated
# attribute-gap comment is allowed; the script cannot associate the comment.
while read -r file; do
    nontest_body "$file" | grep -E 'impl(<[^>]*>)? (serde::)?(Serialize|Deserialize)(<[^>]*>)? for ' \
    | while IFS=: read -r line _rest; do
        prompt "RS-020 $file:$line — candidate: hand-rolled serde impl; verify the adjacent comment naming what attributes cannot express"
    done
done < "$TMP/nontest"

# RS-040 (candidate): tuple type alias with 3+ fields — heterogeneous
# behavior-chain tuples are exempt per gate.md, which the script cannot see.
while read -r file; do
    nontest_body "$file" | grep -E '^[0-9]+:[[:space:]]*(pub )?type [A-Za-z0-9_]+ = \(([^)]*,){2,}' \
    | while IFS=: read -r line _rest; do
        prompt "RS-040 $file:$line — candidate: tuple type alias with 3+ positional fields; named struct unless it's a behavior-chain tuple"
    done
done < "$TMP/nontest"

# RS-060: unwrap() outside tests.
while read -r file; do
    nontest_body "$file" | grep -E '\.unwrap\(\)' | grep -vE '//.*unwrap' \
    | while IFS=: read -r line _rest; do
        finding "RS-060 $file:$line — unwrap() in non-test code; use ?, expect(\"invariant: ...\"), or a typed error"
    done
done < "$TMP/nontest"

# RS-061 (candidate): `let _ =` — the RHS may not be fallible, and a site
# comment can justify ignoring; the script can verify neither.
while read -r file; do
    nontest_body "$file" | grep -E '^[0-9]+:[[:space:]]*let _ = ' \
    | while IFS=: read -r line _rest; do
        prompt "RS-061 $file:$line — candidate: let _ = — if the RHS is fallible, handle/propagate or justify at the site"
    done
done < "$TMP/nontest"

# RS-062 (candidate): allow(dead_code) is legal for an in-progress module; the
# script cannot know the phase.
while read -r file; do
    nontest_body "$file" | grep -E '#\[allow\(dead_code\)\]' \
    | while IFS=: read -r line _rest; do
        prompt "RS-062 $file:$line — candidate: #[allow(dead_code)] — allowed only for an in-progress module; verify it hasn't outlived its phase"
    done
done < "$TMP/nontest"

# RS-033: file-size prompt (counts code before the inline test module —
# a healthy test suite must not push a file over the threshold).
while read -r file; do
    lines=$(nontest_body "$file" | wc -l | tr -d ' ')
    if [ "$lines" -gt "$FILE_FLAG_LINES" ]; then
        prompt "RS-033 $file:1 — $lines non-test lines; look for a real domain boundary"
    fi
done < "$TMP/nontest"

# ---------------------------------------------------------------------------
# Function-level analysis: length (RS-031), duplicate bodies (RS-034),
# free functions with an owning type (RS-030).
# ---------------------------------------------------------------------------
# One awk pass per file emits records:
#   FN <tab> file <tab> line <tab> name <tab> length <tab> toplevel(0/1) <tab> first-param-type <tab> normalized-body
while read -r file; do
    awk -v FILE="$file" '
    function braces(s,  op, cl, t) {
        t = s; op = gsub(/\{/, "", t); t = s; cl = gsub(/\}/, "", t)
        return op - cl
    }
    BEGIN { depth = 0; infn = 0; inimpl = 0 }
    {
        line = $0
        sub(/\/\/.*$/, "", line)                     # strip line comments
        if (!infn && depth == 0 && line ~ /^impl[ <]/) inimpl = 1
        if (!infn && line ~ /(^|[[:space:]])fn [a-zA-Z0-9_]+/ && line !~ /^[[:space:]]*\/\//) {
            infn = 1; fnstart = NR; fndepth = depth; body = ""; opened = 0
            top = (depth == 0 && !inimpl) ? 1 : 0
            # name
            n = index(line, "fn ")
            rest = substr(line, n + 3)
            split(rest, parts, "("); fnname = parts[1]
            # first parameter type: text up to first , or ) after the (
            p = index(rest, "(")
            sig = substr(rest, p + 1)
            split(sig, ps, /[,)]/); first = ps[1]
            sub(/^[[:space:]]*mut[[:space:]]+/, "", first)
            sub(/^[a-z_][a-zA-Z0-9_]*[[:space:]]*:[[:space:]]*/, "", first)
            gsub(/[&[:space:]]/, "", first)
            sub(/^mut/, "", first)
            sub(/<.*/, "", first)
            ptype = first
            # a body opening on the signature line (incl. one-line fns):
            # capture text after the first { so RS-034 sees one-line bodies
            bpos = index(line, "{")
            if (bpos > 0) {
                opened = 1
                btxt = substr(line, bpos + 1)
                sub(/\}[[:space:]]*$/, "", btxt)
                norm = btxt; gsub(/[[:space:]]/, "", norm)
                if (norm != "") body = body norm "\x01"
            }
        } else if (infn && NR > fnstart) {
            # body excludes the signature line so identical bodies with
            # different names still hash equal (RS-034)
            if (!opened && index(line, "{") > 0) opened = 1
            norm = line; gsub(/[[:space:]]/, "", norm)
            if (norm != "") body = body norm "\x01"
        }
        depth += braces(line)
        if (infn && opened && depth <= fndepth) {
            len = NR - fnstart + 1
            printf "FN\t%s\t%d\t%s\t%d\t%d\t%s\t%s\n", FILE, fnstart, fnname, len, top, ptype, body
            infn = 0
        }
        # a bodiless declaration (trait method ending in `;`) is not a function
        if (infn && !opened && line ~ /;[[:space:]]*$/) infn = 0
        if (inimpl && depth == 0 && $0 ~ /\}/) inimpl = 0
    }
    ' "$file"
done < "$TMP/nontest" > "$TMP/fns"

if [ "$DUMP_FNS" -eq 1 ]; then cat "$TMP/fns"; fi

# RS-031: long-function candidates.
awk -F'\t' -v max="$FN_FLAG_LINES" \
    '$1 == "FN" && $5 > max { printf "RS-031 %s:%s — fn %s is %s lines; must read as ordered named steps or be split\n", $2, $3, $4, $5 }' \
    "$TMP/fns" >> "$TMP/prompts"

# RS-034: duplicate normalized bodies.
awk -F'\t' -v min="$MIN_DUP_BODY_LINES" '
    $1 == "FN" && split($8, _lines, "\x01") - 1 >= min {
        key = $8
        if (key in seen) {
            printf "RS-034 %s:%s — fn %s duplicates fn %s (%s); lift to one owner\n", $2, $3, $4, names[key], seen[key]
        } else { seen[key] = $2 ":" $3; names[key] = $4 }
    }' "$TMP/fns" >> "$TMP/findings"

# RS-030: top-level fn whose first param is a crate type that has an inherent impl block.
# Owning types = declared structs/enums that also carry an `impl Type {` block.
#
# Inherent blocks only, never `impl Trait for Type`. The smell RS-030 names is a type whose
# operations are *already* methods acquiring a free function beside them — so the evidence it
# needs is that someone chose methods as that type's interface. A type with only trait impls has
# no method surface to be inconsistent with, and newtypes whose whole interface is
# FromStr/Display/Deref are the common case: calling every function that takes one first a
# misplacement flags ordinary code, and the fix it proposes would move the function onto a type
# that has no business knowing about it.
{
    while read -r file; do
        sed -nE 's/^[[:space:]]*(pub([(][^)]*[)])? )?(struct|enum) ([A-Za-z0-9_]+).*/\4/p' "$file"
    done < "$TMP/files" | sort -u > "$TMP/decls"
    while read -r file; do
        sed -nE 's/^impl(<[^>]*>)? +([A-Za-z0-9_]+)( *<[^>]*>)? *\{.*/\2/p' "$file"
    done < "$TMP/files" | sort -u > "$TMP/impls"
    comm -12 "$TMP/decls" "$TMP/impls" > "$TMP/owners"
}
awk -F'\t' 'NR == FNR { owners[$1] = 1; next }
    $1 == "FN" && $6 == 1 && ($7 in owners) {
        printf "RS-030 %s:%s — free fn %s takes %s as its first parameter; make it a method (or an *Ext trait if %s lives in another crate)\n", $2, $3, $4, $7, $7
    }' "$TMP/owners" "$TMP/fns" >> "$TMP/findings"

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------
COUNT=0
if [ -s "$TMP/findings" ]; then
    sort -t- -k2 "$TMP/findings" | sort -s -t' ' -k1,1 | uniq
    COUNT=$(sort "$TMP/findings" | uniq | wc -l | tr -d ' ')
fi

PROMPTS=0
if [ -s "$TMP/prompts" ]; then
    sort -t- -k2 "$TMP/prompts" | sort -s -t' ' -k1,1 | uniq
    PROMPTS=$(sort "$TMP/prompts" | uniq | wc -l | tr -d ' ')
fi

echo ""
echo "== rust-style mechanical gate: $COUNT violation(s), $PROMPTS candidate prompt(s) across $(wc -l < "$TMP/files" | tr -d ' ') file(s)"
if [ "$PROMPTS" -gt 0 ]; then
    echo "== Candidates (RS-002, 010, 012, 020, 031, 033, 040, 061, 062) have a justified-exception branch"
    echo "==   only the judgment tier can verify; they do not fail this run."
fi
echo "== Coverage — violations: RS-001, 003 (narrow shapes only), 030, 034, 060; candidates: RS-002,"
echo "==   010, 012, 020, 031, 033, 040, 061, 062 (heuristic: single-line signatures/closures;"
echo "==   inline tests assumed after #[cfg(test)])."
# Judgment list below mirrors SKILL.md's coverage statement — keep the two in sync.
echo "== NOT checked here (judgment tier): RS-004, 005, 011, 013, 014, 021, 022, 032, 035, 036, 037,"
echo "==   041, 042, 050, 051, 052, 054, 063, 065; decomposition, cross-crate duplication,"
echo "==   algorithm-level reuse."

[ "$COUNT" -eq 0 ]
