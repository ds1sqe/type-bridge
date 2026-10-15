# Concurrency and conditional writes

TypeDB CE 3.13.6 uses snapshot isolation with optimistic conflict checks at
commit. A condition and its writes belong in the same transaction, but that
alone does not protect data the transaction only reads. Use the schema's
constraints or a deliberate write to the shared record that guards the rule.

The TypeQL examples below run on 3.13.6. They describe server behavior;
transaction and query errors still need handling in your chosen driver.

## What a transaction sees

A transaction opens on a snapshot of committed data. Later commits do not
change that snapshot: a read transaction can keep returning the old value
after another transaction has changed it. Opening a new transaction gives a
new snapshot.

Reads, including `match` inside a write transaction, are never registered for
conflict validation. There is no read lock or serializable isolation option.
A successful read therefore says nothing about whether another transaction
can change that data before your commit.

Data writers do not acquire blocking locks against each other while their
queries run. TypeDB checks the buffered writes and locks at commit. Among
conflicting concurrent transactions, the first successful committer in the
server's write-ahead-log sequence wins. A later conflicting commit fails
with `STC2`; all its writes are rejected. A predecessor that aborts does not
win. Schema/write exclusivity is separate: it is acquired at transaction
open and can time out there.

## What conflicts

TypeDB uses **exclusive** locks for conflicting writes and **unmodifiable**
locks to prevent concurrent deletion of data a write requires. These are
server commit-validation records, not application mutexes. Two unmodifiable
locks on the same data are compatible.

| Operation | Commit-time protection and scope |
| --- | --- |
| Write a `has` edge constrained by `@unique` | Exclusive lock for the attribute value within the constraint's owner-type scope. Concurrent owners cannot acquire the same unique value. |
| Write a `has` edge constrained by `@key` | The uniqueness lock above plus the ownership cardinality lock; `@key` combines uniqueness and exactly-one ownership. |
| Write/delete a `has` edge with a checked `@card` on `owns` | Exclusive lock per owner instance and constrained attribute capability. Unordered `owns` defaults to `@card(0..1)`, so this applies even without an explicit annotation. |
| Write/delete a `links` edge with a checked `@card` on `relates` | Exclusive lock per relation instance and role. Explicit unordered `relates` defaults to `@card(0..1)`. It does not limit how many different relations a player joins. |
| Write/delete a `links` edge with a checked `@card` on `plays` | Exclusive lock per player instance and role. `plays` defaults to unbounded `@card(0..)`, which adds no cardinality lock. |
| Update ordered `has` or ordered `links` | Exclusive lock on the owner's/relation's order property, so concurrent changes to that ordering conflict. |
| Attach an attribute or role player | Unmodifiable locks on persisted owner/relation/player records; an attribute vertex is protected even when put in this transaction. A concurrent delete of required data conflicts, whichever transaction commits second. |
| Remove the last owner of a non-independent attribute | Cleanup can delete the orphan attribute. A concurrent attachment of that same attribute value elsewhere can therefore conflict with the deletion. |
| Create an entity or relation | Creation itself uses a fresh identifier and takes no shared lock. Adding ownerships or players can add the locks described above. |
| Put an inline attribute value | The put itself has no exclusive lock. A `has` edge using that value can still take constraint locks. New large-string/struct values also lock their hash bucket; reusing a persisted hashed attribute protects it against deletion. |
| Read, or run `match` without a protected write | No read lock and no commit validation of the read. Two transactions reading the same rows do not conflict. |

A **checked** cardinality is any bound other than `@card(0..)`. A bound of two
still takes the same exclusive lock: three concurrent additions to a role
with `@card(0..2)` can produce one commit and two conflicts, although a second
addition would be legal after retrying. Different player instances do not
share that player-role lock.

Two puts of the same storage key do not by themselves conflict. The
constraint and deletion locks above are what matter. A same-value update of
a cardinality-constrained ownership still buffers a write and takes its
lock; it is not a no-op for conflict detection. A transaction with neither
writes nor locks cannot have a data isolation conflict.

These rules follow the 3.13.6 server's
[commit-lock construction](https://github.com/typedb/typedb/blob/3.13.6/concept/thing/thing_manager.rs#L1962-L2119)
and [dependency checks](https://github.com/typedb/typedb/blob/3.13.6/storage/record.rs#L136-L197).

## Schema and starting data

Run this schema in a schema transaction, then commit. The role constraint on
`doctor plays assignment:assignee` will be used in the assignment recipe.
The doctor ownerships deliberately rely on the default `owns @card(0..1)`.

```typeql title="schema"
define
attribute item-id, value string;
attribute stock, value integer;
attribute price, value integer;
attribute account-id, value string;
attribute balance, value integer;
attribute purchase-id, value string;
attribute name, value string;
attribute on-call, value boolean;
attribute version, value integer;
attribute shift-id, value string;
attribute lock-key, value string;
entity item, owns item-id @key, owns stock, owns price, plays purchase:item;
entity account, owns account-id @key, owns balance, plays purchase:buyer;
relation purchase, relates buyer, relates item, owns purchase-id @key;
entity doctor, owns name @key, owns on-call, owns version,
  plays assignment:assignee @card(0..1);
entity shift, owns shift-id @key, plays assignment:target;
relation assignment, relates assignee @card(1..1), relates target @card(1..1);
entity lock-row, owns lock-key @key, owns version;
```

Insert the starting data in a write transaction and commit it. Each recipe
starts from this state in its own database; do not run all recipes in
sequence on one database without resetting it.

```typeql title="seed"
insert
$i isa item, has item-id "item-1", has stock 1, has price 40;
$a isa account, has account-id "account-1", has balance 100;
$d1 isa doctor, has name "doctor-a", has on-call true, has version 0;
$d2 isa doctor, has name "doctor-b", has on-call true, has version 0;
$s1 isa shift, has shift-id "shift-1";
$s2 isa shift, has shift-id "shift-2";
$l isa lock-row, has lock-key "doctor-a-guard", has version 0;
```

## Purchase in one query

Put the stock/balance checks, computations and mutations in one write
pipeline, then commit the transaction:

```typeql title="purchase"
match
  $i isa item, has item-id "item-1", has stock $s, has price $p;
  $s > 0;
  $a isa account, has account-id "account-1", has balance $b;
  $b >= $p;
  let $next-stock = $s - 1;
  let $next-balance = $b - $p;
update
  $i has stock == $next-stock;
  $a has balance == $next-balance;
insert
  $purchase isa purchase, links (buyer: $a, item: $i),
    has purchase-id "purchase-1";
```

Computed values in these `update` assignments require `==`; a plain variable
assignment is not the same syntax. If either comparison fails, the match
produces zero rows and the write stages do nothing. A successful commit of
that no-op is a condition failure, not a successful purchase. Inspect the
answer count and the commit result; query success alone is insufficient.

Concurrent buyers of the same item write its `stock` ownership, and buyers
using the same account write its `balance` ownership. Those default
cardinality locks prevent overselling and lost balance updates. Two buyers
that both saw the last item can run their queries successfully, but only
one commits; the other receives `STC2`.

Retry the entire transaction on a fresh snapshot after `STC2`, with bounded
backoff and jitter. Even with stock for several buyers, contention can let
only one commit per racing round. The price here is the price in the buyer's
snapshot: reading it does not protect it from a concurrent price change.
If the rule requires the price to remain unchanged through commit, include
a same-value write of that ownership in the purchase pipeline as well.

Use one stable purchase ID per logical request; different buyers need
different IDs. Keep the same ID when retrying or reconciling that request.

A unique purchase key helps identify an already recorded operation, but it
does not turn an unknown commit outcome into a definite failure. Reconcile
an unknown outcome using that operation key before applying another charge.

## Write skew and a same-value touch

Suppose the rule is “at least one of these two doctors must stay on call.”
Each transaction reads both on-call doctors:

```typeql title="read-on-call"
match $d isa doctor, has on-call true;
select $d;
```

Transaction A sees two rows and takes doctor A off call:

```typeql title="doctor-a-off"
match $d isa doctor, has name "doctor-a";
update $d has on-call false;
```

Transaction B opened before A committed. It also sees two rows and takes
doctor B off call:

```typeql title="doctor-b-off"
match $d isa doctor, has name "doctor-b";
update $d has on-call false;
```

Both commits succeed: their writes affect different owner instances and
the shared reads were never registered. The final state has no doctor on
call. This is **write skew**, even though each transaction was atomic and
checked the rule inside its own snapshot.

To make these two operations conflict, both transactions can first touch
the same ownerships they rely on, without changing the values:

```typeql title="touch-on-call"
match $d isa doctor, has on-call $value;
update $d has on-call == $value;
```

Run the touch and the chosen doctor's update in the **same** transaction.
With both doctors touched, one racing commit succeeds and the other gets
`STC2`; the rejected transaction must start again and re-check the rule.
The touch only protects the records it matches. It does not lock an absent
row, prevent a new matching record, or make arbitrary queries serializable.

When the rule has one stable anchor, a touch of that shared anchor can be
more focused. A pre-existing guard row can serve that purpose:

```typeql title="touch-guard"
match $l isa lock-row, has lock-key "doctor-a-guard", has version $value;
update $l has version == $value;
```

Every writer enforcing that rule must touch the same guard row in the
transaction that does the protected work. Reading the guard is insufficient.

## Use cardinality on plays to serialize assignments

The schema above constrains how many assignments each doctor can join.
Two transactions can try assigning doctor A to different shifts:

```typeql title="assign-first-shift"
match $d isa doctor, has name "doctor-a";
      $s isa shift, has shift-id "shift-1";
insert $a isa assignment, links (assignee: $d, target: $s);
```

```typeql title="assign-second-shift"
match $d isa doctor, has name "doctor-a";
      $s isa shift, has shift-id "shift-2";
insert $a isa assignment, links (assignee: $d, target: $s);
```

If both transactions opened before either committed, both queries can
succeed, then one commit wins and the other gets `STC2`. A new transaction
that tries to add another assignment after the winner is visible instead
fails the cardinality check at commit with `CNT5`.

The constraint belongs on **plays** for this rule. A `relates` cardinality
limits players within one assignment relation; separate new assignments
have different relation instances and do not share that lock. Without the
plays constraint, read-check-insert can assign the doctor twice. To limit
assignments per shift instead, constrain the shift's corresponding plays
capability. Higher checked bounds also serialize concurrent additions and
can require retries even when another assignment would fit.

## Use keys for uniqueness

The purchase relation has a key on `purchase-id`. Two concurrent
transactions inserting the same key can both pass their query phase, but
only one can commit:

```typeql title="keyed-purchase"
match $i isa item, has item-id "item-1";
      $a isa account, has account-id "account-1";
insert $p isa purchase, links (buyer: $a, item: $i),
  has purchase-id "purchase-key";
```

The loser gets `STC2` at commit. If the winning key was already visible
when a later transaction opened, the duplicate insert instead fails during
query execution with `CNT9` (`@unique`). Handle both phases. Counting
existing purchases before inserting a different key does not enforce a
limit: that count is only a snapshot read.

## An external lock row needs a lease

Acquiring a key in one transaction, doing the work in another, and releasing
it in a third can coordinate callers that all follow the protocol. For
example, acquisition inserts a key and commits:

```typeql title="acquire-external-lock"
insert $l isa lock-row, has lock-key "doctor-a-external", has version 0;
```

Release later deletes that row in another committed transaction:

```typeql title="release-external-lock"
match $l isa lock-row, has lock-key "doctor-a-external";
delete $l;
```

If the holder crashes after acquisition, TypeDB does not expire the row.
Every later acquisition sees a uniqueness violation once the key is visible.
The rule is blocked until someone deletes the stale row. A safe protocol
needs lease expiry, atomic takeover and fencing in the work transaction so
an expired holder cannot continue writing. The two snippets above do **not**
implement that protocol.

Inserting and deleting a lock row inside one transaction also does not
serialize callers: with no surviving write, there is no protecting lock.
A touch of a persistent guard in the work transaction avoids holding a row
across transactions and leaves no extra lock behind after a crash or abort.

## Commit failures and retries

| Failure | What is known | What to do |
| --- | --- | --- |
| `STC2` isolation conflict | The transaction definitely did not commit; the server records the abort before returning this error. | Re-run the whole transaction, including reads and conditions, in a new snapshot with bounded backoff. |
| `CNT9` for an already visible unique key | The insert was rejected during query execution. | Interpret the key: an existing business operation may already be complete; a lock acquisition may need to wait. Do not blindly repeat an operation with a new key. |
| `CNT5` for an already exceeded cardinality | The constraint rejected the transaction. | Re-check the business rule; this is not evidence that a retry can make the same invalid write legal. |
| `STC1`, `STC5`, `STC6`, durability-client `DUC*`, `DCT5`, or post-commit schema errors such as `SCT3`/`SCT4`/`SCT5`/`SCT7` | **Outcome unknown**: writes may already be applied, or recovery may apply a pending WAL record. Some schema error codes also occur before commit. | Reconcile the operation's durable result before retrying its effects. A server error response alone does not prove an abort. |
| Transport loss or timeout while committing | **Outcome unknown**: the server can apply the commit before the connection fails or the response arrives. | Reconcile by a stable operation key. A fresh transaction may inspect committed state; the error itself cannot resolve the outcome. |

After an unknown outcome, an absent result in one fresh snapshot does not
prove that an in-flight or recoverable commit will never apply. Preserve
the operation key while reconciling; do not turn uncertainty into a second
operation with a different key.

A safe retry after `STC2` repeats database work, not any external effects
already performed by the application. Keep payment calls, messages and
other irreversible effects out of a transaction retry loop unless they
have their own idempotency protocol.

The 3.13.6 [storage commit pipeline](https://github.com/typedb/typedb/blob/3.13.6/storage/storage.rs#L268-L345)
can fail while recording an already applied commit's status. Later
[transaction cleanup](https://github.com/typedb/typedb/blob/3.13.6/database/transaction.rs#L499-L617)
can also fail after data or schema commits. These are why an error must not
be treated as synonymous with rollback.

## Why the conflicting winner is not reported

At validation, the server has the predecessor's commit sequence, commit
record, conflicting key and lock types. It discards those details when it
constructs the isolation error. The conflict enum has only a reason, the
protocol carries code/domain/text without winner fields, and the commit
response contains no transaction or commit identifier.

Consequently a client cannot report the winning transaction, user, query
or key from `STC2`. A proxy observing writers can suggest overlapping
candidates, but cannot prove the winner from the error. Reporting a precise
winner requires changes to the server, protocol and driver.

## Using TypeBridge transactions

TypeBridge transaction examples are coming in 2.3.
