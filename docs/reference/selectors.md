# Selector Reference

Pactree selectors are parsed and evaluated by `src/selector.rs`. They operate
on native component trees or serialized projection adapters and return
`SelectorMatch` records with component id, `componentType`, optional `type`,
role, offset, `pact_depth`, path, and content.

Selectors are comma-union expressions over chained simple selectors.

## Evaluation Model

- Input is converted into `PactTreeNode`.
- Children are sorted by `offset`, `created_at_ns`, `creation_index`, then `id`.
- The first simple selector searches all nodes unless it is position-based.
- Later selectors traverse from previous matches using a combinator.
- Union branches are evaluated left-to-right and de-duplicated by node id.
- Top-level controls apply after matching.

## Basic Forms

| Form | Meaning |
| --- | --- |
| `d-1`, `d0`, `d1` | Select depth anchor nodes with matching `pact_depth`. |
| `(d0,0)` | Select child at offset `0` below the `d0` anchor. |
| `(d0..d*,0)` | For each depth anchor from `d0` through max depth, select child offset `0`. |
| `((d0,0), (d1,1))` | Coordinate set union inside one simple selector. |
| `.text_content` | Match `componentType` or `type`. |
| `.b`, `.c`, `.seg` | Aliases for block, container, and segment groups. |
| `.block:tool_result` | Type selector plus required type tag. |
| `#node_id` | Match exact node id. |
| `{role='assistant'}` | Attribute filter. |
| `[priority>=3]` | Bracket filter with comparisons and behavior filters. |
| `+urgent` | Tag filter. |
| `*` | Wildcard simple selector. |

Depth anchors are nodes whose `componentType` or `type` is one of
`system_header`, `message_turn`, `active_message`, or
`active_provider_message`.

## Combinators

| Form | Meaning |
| --- | --- |
| `>` | Direct children only. |
| `?` | Direct children. In current code `?` is a single-step child combinator, not optional matching. |
| whitespace | Descendants at any depth, same as `??`. |
| `??` or `~` | Descendants at any depth. |
| `??within(N)` | Descendants no more than `N` levels below the current node. |

Examples:

```text
d0 > .text_content
#history .text_content
#history ~ .text_content
#history ??within(2) .text_content
```

## Attribute Operators

Curly and bracket filters support:

```text
=   equal
!=  not equal
~=  regex
<   numeric less than
<=  numeric less than or equal
>   numeric greater than
>=  numeric greater than or equal
```

Values may be quoted with single or double quotes. Numeric comparisons parse
numbers from JSON numbers or numeric strings. Invalid regex patterns simply do
not match.

## Behavior Filters

Bracket filters also recognize:

- `ttl`
- `cad` / `cadence`
- `ephemeral`
- `sticky`

`[ephemeral]` matches `ttl == 1`. `[sticky]` matches `ttl == 1` and
`cadence/cad == 1`. Boolean forms such as `[sticky=false]` are accepted.

## Pseudos

Supported structural pseudos:

- `:first` / `:first-child`
- `:last` / `:last-child`
- `:nth(N)`, one-based
- `:has(simple-selector)`

Pseudos can be written after `:` or in the type-tag slot when they take an
argument.

## Controls

Top-level controls are stripped before parsing and applied to the final match
list:

```text
@offset=N
@limit=N
@mode=all
@mode=latest
```

`@mode` currently validates only `all` and `latest`; it does not otherwise
change matching behavior.

## Valid Examples

```text
d0
d1{role='assistant'}
(d0,0)
(d0..d*,0)
((d0,0), (d1,1))
#active_text, #shell
d0 > .text_content
#history .block @offset=1 @limit=1 @mode=latest
.message_turn[priority>=3]
.message_turn{author~='^Earl'}
.note[sticky cadence=1]
+tool_call+urgent
.message_turn:has(.tool_result)
```

## Invalid Examples

```text
^seq .text_content
^ah .text_content
d
(d0,)
{role}
#
>
#history ??within(0) .text_content
```

Caret symbolic region selectors are explicitly rejected; use `d-1`, `d0`,
`d1`, or coordinate selectors instead.

## Source Anchors

- `src/selector.rs`
- `tests/cli.rs`
