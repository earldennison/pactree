# Selector Syntax Cheat Sheet

This is the quick "what do I type?" guide for Pactree selectors. The complete
reference is [`selectors.md`](selectors.md). The source of truth is
`src/selector.rs`.

Selectors are comma-union expressions made from simple selectors joined by
combinators:

```text
simple
simple combinator simple
simple, simple, simple
simple @offset=1 @limit=5
```

## Common Recipes

| Goal | Selector |
| --- | --- |
| Active depth anchor | `d0` |
| System/setup depth anchor | `d-1` |
| History depth anchor | `dN`, where `N` is a number greater than `0`. |
| All text content under active depth | `d0 .text_content` |
| Direct text children of active depth | `d0 > .text_content` |
| Any tool result below a turn | `.message_turn .tool_result` |
| Node by id | `#node_id` |
| Assistant turns | `.message_turn{role='assistant'}` |
| Tagged nodes | `+urgent` |
| Nodes with two tags | `+tool_call+urgent` |
| First matching text block | `.text_content:first` |
| Last matching text block | `.text_content:last` |
| Limit result count | `.text_content @limit=10` |
| Page through results | `.text_content @offset=10 @limit=10` |

## Simple Selectors

Simple selectors can be combined without spaces:

```text
.message_turn{role='assistant'}[priority>=3]+important:last
```

| Form | Meaning |
| --- | --- |
| `*` | Match any component. |
| `dN` | Match depth anchors with that `pact_depth`; `N` is a signed number such as `-1`, `0`, or `3`. |
| `.text_content` | Match `componentType` or `type`. |
| `.block:tool_result` | Match type plus a required type tag. |
| `#node_id` | Match exact id. |
| `+tag` | Match tag. Tags may contain `:` and `*`, for example `+shell:*`. |
| `{role='user'}` | Attribute filter. Comma-separated inside `{}`. |
| `[priority>=3]` | Bracket filter. Space or comma separated inside `[]`. |
| `:first`, `:last`, `:nth(2)` | Structural pseudo selector. |
| `:has(.tool_result)` | Match nodes containing a simple selector. |

Identifier selectors accept letters, numbers, `_`, and `-`. Scoped ids may use
one `:` when the local id is an 8-character hex suffix. Tag selectors are looser
and also accept `:` and `*`.

## Combinators

| Form | Meaning |
| --- | --- |
| `A > B` | Direct children of `A`. |
| `A ? B` | Direct children of `A`. |
| `A ?? B` | Descendants of `A` at any depth. |
| `A ~ B` | Same as `??`. |
| `A B` | Same as `??`. Whitespace is descendant search. |
| `A ??within(2) B` | Descendants no more than 2 levels below `A`. |

`within(N)` must follow `?` or `??`, and `N` must be greater than zero.

## Position Selectors

Position selectors target components by depth anchor and child offsets.

```text
(d0,0)
(d1,first)
(d0,last)
(d0,0..2)
(d0,all)
(d0..d*,0)
((d0,0), (d1,1))
```

| Form | Meaning |
| --- | --- |
| `(d0,0)` | Child at offset `0` below depth `0`. |
| `(d0,first)` | First child below depth `0`. |
| `(d0,last)` | Last child below depth `0`. |
| `(d0,0..2)` | Children from offset `0` through `2`. |
| `(d0,all)` | All children below depth `0`. |
| `(d0..d2,0)` | Offset `0` under each depth from `0` through `2`. |
| `(d0..d*,0)` | Offset `0` under each depth from `0` through max depth. |
| `((d0,0), (d1,1))` | Coordinate-set union in one simple selector. |

Coordinate aliases:

```text
first == min == -last
last  == max == +last
all   == +all
+3    == 3
```

## Attribute Filters

Curly filters use comma-separated attribute expressions:

```text
.message_turn{role='assistant', author~='^Earl'}
```

Bracket filters use comma or whitespace:

```text
.message_turn[priority>=3 sticky=false]
```

Operators:

```text
=    equal
!=   not equal
~=   regex
<    numeric less than
<=   numeric less than or equal
>    numeric greater than
>=   numeric greater than or equal
```

Values may be quoted with single or double quotes. Numeric comparisons parse
JSON numbers or numeric strings.

## Behavior Filters

Bracket filters also understand lifecycle behavior names:

| Form | Meaning |
| --- | --- |
| `[ttl]` | Has a TTL-like field. |
| `[cad]` or `[cadence]` | Has cadence. |
| `[ephemeral]` | TTL is `1`. |
| `[sticky]` | TTL is `1` and cadence is `1`. |
| `[sticky=false]` | Negated behavior check. |

## Pseudos

| Form | Meaning |
| --- | --- |
| `:first`, `:first-child` | First sibling among the candidate group. |
| `:last`, `:last-child` | Last sibling among the candidate group. |
| `:nth(N)` | One-based nth match. |
| `:has(simple)` | Node has a descendant matching `simple`. |

Pseudos can be used directly or after a type selector:

```text
.text_content:first
.message_turn:has(.tool_result)
```

## Controls

Controls are top-level suffixes. They are ignored inside quotes, `{}`, `[]`, and
`()`.

```text
@offset=N
@limit=N
@mode=all
@mode=latest
```

`@offset` skips matches. `@limit` truncates matches. `@mode` currently validates
`all` and `latest`.

## Valid Examples

```text
d0
d1{role='assistant'}
(d0,0)
(d0..d*,0)
((d0,0), (d1,1))
#active_text, #shell
d0 > .text_content
d0 .text_content
#history ??within(2) .text_content
#history .block @offset=1 @limit=1 @mode=latest
.message_turn[priority>=3]
.message_turn{author~='^Earl'}
.note[sticky cad=1]
+tool_call+urgent
.message_turn:has(.tool_result)
```

## Gotchas

- `^seq`, `^ah`, and other caret region selectors are rejected. Use depth
  selectors such as `d-1`, `d0`, and `dN`.
- Whitespace between simple selectors means recursive descendant search, not
  direct child search. Use `>` or `?` for direct children.
- `within(N)` must follow `?` or `??`.
- `??within(0)` is invalid.
- A selector cannot start or end with a combinator.
- Top-level commas create union branches and de-duplicate results by node id.
