# The event graph in SurrealDB

What the syncer writes, as far as a plugin's query needs it. The full
description is in the syncer's README
(https://github.com/gyorgybalazsi/canton-event-graph-syncer/blob/main/syncer/README.md,
section "Data model"); check it if a field here seems missing.

## Records

| Table | Record id | Fields a plugin usually reads |
|---|---|---|
| `created` | `created:<contract_id>` | `contract_id`, `template_name` (`Module.Entity`), `template { package_name, module_name, entity_name, package_id }`, `signatories`, `witness_parties`, `offset`, `node_id`, `created_at`, `create_arguments { .. }`, `from_acs` |
| `exercised` | `exercised:[<offset>, <node_id>]` | `choice_name`, `template { .. }`, `interface { .. }` (interface choices), `target_contract_id`, `acting_parties`, `witness_parties`, `consuming`, `result_contract_ids`, `offset`, `node_id`, `transaction_effective_at`, `choice_argument { .. }` |
| `transaction` | `transaction:<offset>` | `offset`, `update_id`, `command_id`, `workflow_id`, `effective_at`, `record_time` |
| `party` | `party:<party_id>` | `party_id`, `name` (the part before `::`) |
| `sync_state` | `sync_state:stream` | `last_offset`: the last offset fully written. The high-water query reads it. |

Relations (graph edges): `action` (transaction to its root events),
`consequence` (exercise to its child events), `target` (exercise to the
contract it was exercised on), `consumes` (consuming exercise to the
contract it archived), `requested` (party to the transaction it submitted).
Reading a field through a record id, as below, is usually simpler and faster
than walking an edge.

Notes:

- `offset` is the ledger offset of the transaction; every event of one
  transaction has the same offset. Plugins page by it.
- Contracts loaded from the ACS at a fresh start have `offset = -1`,
  `from_acs = true`, and no exercise that created them. A step's range never
  includes them.
- `template_name` is `Module.Entity` without the package. Filter on
  `template.entity_name` (plus `template.module_name` and
  `template.package_name` when needed), not on `package_id`: the id changes
  with every package version, the name does not.
- For an interface choice, `interface { .. }` names the interface and
  `template { .. }` the contract's template.
- `result_contract_ids` holds every contract id in the choice's return value.

## Daml values

`create_arguments` and `choice_argument` keep the shape of the Daml value:

| Daml | Stored as | Declare the input column as |
|---|---|---|
| `Party`, `Text`, `ContractId` | string | `Utf8` |
| `Int` | integer | `Int64` |
| `Numeric n` / `Decimal` | decimal | `Float64` (the daemon converts) |
| `Bool` | bool | `Boolean` |
| `Time` | datetime | `Timestamp(Microsecond, Some("UTC"))` |
| `Date` | datetime at midnight UTC | `Timestamp(Microsecond, Some("UTC"))` |
| Record | object | read its fields by path, each `AS` a column |
| Enum | the constructor name, a string | `Utf8` |
| Variant | `{ constructor, value }` | `x.constructor AS kind`, `x.value AS ..` |
| Optional | the value, or null | the value's type; nulls are fine |
| List | array | `List(Utf8)` for a list of parties or texts |
| TextMap | object | read keys by path |
| GenMap | array of `{ key, value }` | usually avoid; aggregate in SurrealQL if needed |

The daemon turns each row into Arrow by field name, with the column types
the plugin declared: a missing or `NONE` field becomes null, an integer is
accepted for a float column, a datetime string for a timestamp column. Any
other mismatch fails the step with the column's name.

## Useful query shapes

```sql
-- A choice, with a field of the contract it was exercised on.
SELECT offset, transaction_effective_at AS at,
    type::record('created', target_contract_id).create_arguments.amount AS amount
    FROM exercised WHERE choice_name = 'Settle' AND template.entity_name = 'Trade'
    AND offset > $from AND offset <= $to;

-- A choice, with a field of the contract it returned.
SELECT offset, transaction_effective_at AS at,
    type::record('created', result_contract_ids[0]).create_arguments.quantity AS quantity
    FROM exercised WHERE choice_name = 'Accept' AND template.entity_name = 'Proposal'
    AND offset > $from AND offset <= $to;

-- Creates of a template, with a nested field and a party.
SELECT offset, created_at AS at, create_arguments.issuer AS issuer,
    create_arguments.terms.notional AS notional
    FROM created WHERE template.entity_name = 'Bond'
    AND offset > $from AND offset <= $to;

-- A choice argument.
SELECT offset, transaction_effective_at AS at, choice_argument.quantity AS quantity
    FROM exercised WHERE choice_name = 'Split' AND template.entity_name = 'Holding'
    AND offset > $from AND offset <= $to;
```

To try one by hand, put numbers in for the range, for example
`LET $from = 0; LET $to = 1000;` before it in `surreal sql`, or write the
numbers in place in the explorer's query panel.
