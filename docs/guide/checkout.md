# Checkout: a whole cart in one query

A shop sells items from stock to accounts that pay from a balance. A
checkout takes a whole cart. Every line must be in stock and the account
must cover the total. Then every stock, the balance and the order lines
change together, or nothing changes.

This page builds that checkout with a staged transaction. The same program
runs in Python, TypeScript, Rust and C, directly and through the proxy. It
is the `shop` program of the `writes` conformance suite, so its plans, answers
and stored state are compared across all four languages in CI.

## Why one query

The obvious version reads first and writes later. With two buyers of the last
book:

```text
buyer A  read   book stock 1          check passes
buyer B  read   book stock 1          check passes
buyer A  write  book stock 0, order
buyer B  write  book stock 0, order   the book is sold twice
``` A transaction alone does not fix this when the check
only reads (see [what conflicts](concurrency.md#what-conflicts)).

Put the check and the write in the same query instead. The query matches the
account and every item, compares the stock and the balance, computes the new
values, and writes them, all in one TypeQL pipeline on one snapshot.
Concurrent buyers write the same `stock` or `balance`, so TypeDB lets only
one of them commit; the other gets a conflict it can retry.

## The cart as one plan

Each cart line adds an item binding, a stock check and its cost to the
total. After the loop, one more check compares the balance with the total.
Then the plan writes every new stock, the new balance and one `order-line`
per item:

```python title="Python"
--8<-- "examples/shop/python/program.py:checkout"
```

=== "TypeScript"

    ```ts
    --8<-- "examples/shop/ts/program.ts:checkout"
    ```

=== "Rust"

    ```rust
    --8<-- "examples/shop/rust/main.rs:checkout"
    ```

=== "C (ABI 1.7)"

    ```c
    --8<-- "examples/shop/c/program.c:checkout"
    ```

For the cart `pen x2, book x1` this plan becomes one query of this shape:

```typeql
match
  $buyer isa! account, has account-id "A1", has balance $balance;
  $item isa! item, has sku "pen", has stock $stock, has price $price;
  $stock >= 2;
  $item_2 isa! item, has sku "book", has stock $stock_2, has price $price_2;
  $stock_2 >= 1;
  let $total = $price * 2 + $price_2 * 1;
  $balance >= $total;
  let $stock_1 = $stock - 2;
  let $stock_2_after = $stock_2 - 1;
  let $balance_after = $balance - $total;
update
  $item has stock == $stock_1;
  $item_2 has stock == $stock_2_after;
  $buyer has balance == $balance_after;
insert
  (buyer: $buyer, item: $item) isa order-line, has order-id "O1", has quantity 2;
  (buyer: $buyer, item: $item_2) isa order-line, has order-id "O1", has quantity 1;
```

The variable names are illustrative; the plan derives its own.

## Stage, then commit once

A staged transaction sends nothing while you stage. `commit` runs every
staged plan in one TypeDB transaction and commits it, and only then does each
plan's answer arrive:

```python title="Python"
--8<-- "examples/shop/python/program.py:stage"
```

Through the proxy, the whole transaction is one request.

## Read the answer

A failed check is not an error. If any line is out of stock, or the balance
is short, the match is empty: nothing is written, the commit succeeds, and
the plan answers `Matched(0)`. A completed checkout answers `Matched(1)`.
Always check it.

The program runs these steps, each in its own transaction:

| Step | Cart | Answer | Why |
| --- | --- | --- | --- |
| checkout | A1: pen x2, book x1 (46 of 100) | `Matched(1)` | in stock, balance covers it |
| sold out | A2: mug x1, book x1 | `Matched(0)` | the book is sold out, so the mug is not taken either |
| short | A2: mug x3 (36 of 30) | `Matched(0)` | the balance is short |
| two carts | A2: mug x1 (12), then pen x6 (18) | `Matched(1)`, `Matched(1)` | the second cart sees the first one's charge |
| race | eight buyers, one lamp in stock | one `Matched(1)`, seven `Matched(0)` | see [when the commit fails](#when-the-commit-fails) |

## One plan or several

Plans staged in one transaction commit together, and each one sees the
writes staged before it: in the last step, the pen cart sees a balance of 18
after the mug cart's charge. But each plan's check is its own. A plan that
matches nothing does not stop the others, and they still commit.

So:

- Put every check that must gate the purchase in the same plan as its
  writes. A cart is one plan.
- Stage several plans in one transaction when each is complete on its own and
  they should commit together, such as two independent carts or a cart and an
  audit record that does not depend on it.

## When the commit fails

A lost race is a typed conflict at commit, and nothing from the transaction
is stored. When `retry_safe` is true, run the whole transaction again; it
reads a fresh snapshot, so the retried checkout sees the stock the winner
left.

The program's race step shows it. Eight buyers check out the last lamp at
the same moment, each in its own transaction. Every buyer stages first, then
all commit together:

```python title="Python"
--8<-- "examples/shop/python/program.py:race"
```

One commit wins. The other seven lose with a retry-safe conflict, run again,
find the lamp sold out and answer `Matched(0)`. The sum of the answers is
exactly 1, the lamp's stock is 0, the account is charged once and there is
one order line, in every language and through the proxy. [Commit, and retry a conflict](concurrency.md#commit-and-retry-a-conflict)
shows the loop, and [Typed failures](concurrency.md#typed-failures) lists the
outcomes, including the unknown outcome a proxy can report.

## Run it

The programs are `examples/shop/{python,ts,rust,c}`, and their schema is the
workspace in `tests/contracts/sdk_conformance/writes/shop`. Against a TypeDB
3.13.6 server:

```bash
uv run python scripts/ci/run_sdk_conformance.py writes \
    --semantic-profile typedb-3.13.6/v1 --address localhost:1729 \
    --http-port 8000 --output tmp/sdk-writes
```

The runner generates the four packages, runs every writes program directly
and through a proxy, and compares their reports.
