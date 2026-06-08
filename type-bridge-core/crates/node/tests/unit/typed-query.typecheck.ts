// Compile-only negative checks for the typed query surface. Compiled by
// tsconfig.unit but never executed (only `*.test.js` runs). Each
// `@ts-expect-error` line must stay an error: if a regression makes the case
// compile, the unit `tsc` build fails.

import { agg, attr } from "../../typescript/index.js";

class Name extends attr.String("q-name") {}
class Email extends attr.String("q-email") {}
class Age extends attr.Integer("q-age") {}
class Score extends attr.Double("q-score") {}

// Positives: these must compile.
Age.gt(new Age(18n));
Age.gte(new Age(18n)).and_(Score.gt(new Score(90)));
Name.eq(new Name("Alice"));
Name.startsWith("Al");
Name.contains("li");
Age.avg();
Score.sum();
agg.count();
Age.asc();
Name.desc();

// @ts-expect-error wrong-branded value: Name instance into an Age comparison
Age.gt(new Name("x"));
// @ts-expect-error raw primitive is not a branded attribute
Name.gt(5);
// @ts-expect-error same value type, distinct brand (Email vs Name)
Name.eq(new Email("a@b.c"));
// @ts-expect-error string operator on a non-string attribute
Score.like("x");
// @ts-expect-error string operator on a non-string attribute
Age.startsWith("1");
// @ts-expect-error aggregate helper on a string attribute
Name.sum();
// @ts-expect-error string-op argument must be a string literal
Name.startsWith(5);
