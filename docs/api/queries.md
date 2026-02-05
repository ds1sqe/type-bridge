# Queries

Complete reference for query expressions, filtering, aggregations, and pagination in TypeBridge.

## Overview

TypeBridge provides a fully type-safe expression-based query API for advanced filtering, aggregations, and boolean logic. All expressions are validated at compile-time by type checkers and execute efficiently on the database.

**New in v0.6.0**: Chainable delete and update operations now support all expression-based filters. See [Chainable Operations](#chainable-operations) and [CRUD Operations](crud.md) for details.

## Field References

Access entity fields at the class level to get type-safe field references for query building:

```python
from type_bridge import Entity, TypeFlags, String, Integer

class Name(String):
    pass

class Age(Integer):
    pass

class Email(String):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age
    email: Email

# Class-level access returns FieldRef for query building
Person.age      # Returns NumericFieldRef[Age]
Person.name     # Returns StringFieldRef[Name]
Person.email    # Returns StringFieldRef[Email]

# Instance-level access returns attribute values
person = Person(name=Name("Alice"), age=Age(30))
person.age      # Returns Age(30) instance
person.name     # Returns Name("Alice") instance
```

## Value Comparisons

Filter entities using type-safe comparison operators on numeric fields:

### Greater Than / Less Than

```python
# Greater than
older = manager.filter(Person.age.gt(Age(30)))

# Less than
younger = manager.filter(Person.age.lt(Age(25)))

# Greater than or equal
adults = manager.filter(Person.age.gte(Age(18)))

# Less than or equal
youth = manager.filter(Person.age.lte(Age(25)))
```

### Equality and Inequality

```python
# Equal to
exact = manager.filter(Person.age.eq(Age(30)))

# Not equal to
not_thirty = manager.filter(Person.age.neq(Age(30)))
```

### Range Queries

Combine multiple comparisons for range queries (implicit AND):

```python
# Age between 18 and 65 (exclusive)
working_age = manager.filter(
    Person.age.gte(Age(18)),
    Person.age.lt(Age(65))
)

# Salary range
mid_range = manager.filter(
    Person.salary.gte(Salary(50000)),
    Person.salary.lte(Salary(100000))
)
```

### Available Comparison Methods

All numeric field references support:

- `.gt(value)` - Greater than
- `.lt(value)` - Less than
- `.gte(value)` - Greater than or equal
- `.lte(value)` - Less than or equal
- `.eq(value)` - Equal to
- `.neq(value)` - Not equal to

## String Operations

Perform text searches with type-safe string methods:

### Contains Substring

```python
# Find emails containing "@gmail.com"
gmail_users = manager.filter(Person.email.contains(Email("@gmail.com")))

# Find names containing "Alice"
alice_variants = manager.filter(Person.name.contains(Name("Alice")))
```

### Regex Pattern Matching

```python
# Names starting with "A"
a_names = manager.filter(Person.name.like(Name("^A.*")))

# Emails matching pattern
pattern_emails = manager.filter(Person.email.like(Email(".*@(gmail|yahoo)\\.com")))

# Regex (alias for like)
city_pattern = manager.filter(Person.city.regex(City("New.*")))
```

### Available String Methods

All string field references support:

- `.contains(value)` - Substring match
- `.like(value)` - Regex pattern (TypeQL `like`)
- `.regex(value)` - Regex pattern (alias for `like`)

## Boolean Logic

Compose complex queries with AND, OR, NOT operators:

### OR Logic

```python
# Young OR senior
young_or_old = manager.filter(
    Person.age.lt(Age(25)).or_(Person.age.gt(Age(60)))
)

# Multiple OR conditions
special_cases = manager.filter(
    Person.status.eq(Status("vip")).or_(
        Person.years_active.gt(Years(10))
    ).or_(
        Person.contribution.gt(Score(1000))
    )
)
```

### AND Logic (Explicit)

```python
# Senior engineers (explicit AND)
senior_engineers = manager.filter(
    Person.department.eq(Department("Engineering")).and_(
        Person.job_title.contains(JobTitle("Senior"))
    )
)

# Complex AND chain
qualified = manager.filter(
    Person.age.gte(Age(25)).and_(
        Person.experience.gte(Years(5))
    ).and_(
        Person.education.eq(Education("Masters"))
    )
)
```

### NOT Logic

```python
# Not in sales department
non_sales = manager.filter(
    Person.department.eq(Department("Sales")).not_()
)

# Not matching pattern
non_admins = manager.filter(
    Person.role.like(Role("admin.*")).not_()
)
```

### Implicit AND

Multiple filters without boolean operators are implicitly AND'ed:

```python
# All filters are AND'ed together
result = manager.filter(
    Person.age.gt(Age(18)),           # AND
    Person.age.lt(Age(65)),           # AND
    Person.status.eq(Status("active")) # AND
).execute()
```

### Complex Boolean Expressions

```python
# (age > 40 AND salary > 100k) OR performance > 90
top_talent = manager.filter(
    Person.age.gt(Age(40)).and_(
        Person.salary.gt(Salary(100000.0))
    ).or_(
        Person.performance.gt(Performance(90.0))
    )
)

# ((department = "Engineering" AND level >= "Senior") OR years > 10) AND active = true
experienced = manager.filter(
    Person.department.eq(Department("Engineering")).and_(
        Person.level.gte(Level("Senior"))
    ).or_(
        Person.years.gt(Years(10))
    ).and_(
        Person.active.eq(Active(True))
    )
)
```

## Database-Side Aggregations

Execute efficient aggregations on the database (not in Python):

### Single Aggregation

```python
# Average age
result = manager.filter().aggregate(Person.age.avg())
avg_age = result['avg_age']
print(f"Average age: {avg_age}")

# Sum of salaries
result = manager.filter().aggregate(Person.salary.sum())
total = result['sum_salary']
print(f"Total payroll: ${total:,.2f}")
```

### Multiple Aggregations

```python
# Multiple statistics in one query
stats = manager.filter(
    Person.department.eq(Department("Engineering"))
).aggregate(
    Person.age.avg(),
    Person.salary.avg(),
    Person.salary.sum(),
    Person.salary.max(),
    Person.salary.min()
)

# Access results
print(f"Average age: {stats['avg_age']}")
print(f"Average salary: ${stats['avg_salary']:,.2f}")
print(f"Total payroll: ${stats['sum_salary']:,.2f}")
print(f"Max salary: ${stats['max_salary']:,.2f}")
print(f"Min salary: ${stats['min_salary']:,.2f}")
```

### Available Aggregation Methods

All numeric field references support:

- `.avg()` - Average value
- `.sum()` - Sum of values
- `.max()` - Maximum value
- `.min()` - Minimum value
- `.median()` - Median value
- `.std()` - Standard deviation

## Group-By Queries

Group entities by field values and compute per-group aggregations:

### Group by Single Field

```python
# Average salary by department
dept_stats = manager.group_by(Person.department).aggregate(
    Person.salary.avg()
)

# Results: dict mapping group values to stats
for dept, stats in dept_stats.items():
    print(f"{dept}: avg salary = ${stats['avg_salary']:,.2f}")

# Example output:
# Engineering: avg salary = $95,000.00
# Sales: avg salary = $75,000.00
# Marketing: avg salary = $68,000.00
```

### Group by Multiple Fields

```python
# Average salary by job title and department
title_dept_stats = manager.group_by(
    Person.job_title,
    Person.department
).aggregate(Person.salary.avg())

# Results: dict with tuple keys
for (title, dept), stats in title_dept_stats.items():
    print(f"{title} in {dept}: avg salary = ${stats['avg_salary']:,.2f}")

# Example output:
# Engineer in Engineering: avg salary = $90,000.00
# Senior Engineer in Engineering: avg salary = $120,000.00
# Manager in Engineering: avg salary = $150,000.00
```

### Group with Multiple Aggregations

```python
# Multiple stats per group
dept_stats = manager.group_by(Person.department).aggregate(
    Person.age.avg(),
    Person.salary.avg(),
    Person.salary.sum(),
    Person.experience.avg()
)

for dept, stats in dept_stats.items():
    print(f"{dept}:")
    print(f"  Avg age: {stats['avg_age']:.1f}")
    print(f"  Avg salary: ${stats['avg_salary']:,.2f}")
    print(f"  Total payroll: ${stats['sum_salary']:,.2f}")
    print(f"  Avg experience: {stats['avg_experience']:.1f} years")
```

## Combining Filters and Aggregations

Chain expression filters with aggregations:

```python
# Filter THEN aggregate
eng_stats = manager.filter(
    Person.department.eq(Department("Engineering")),
    Person.age.gt(Age(30))
).aggregate(
    Person.salary.avg(),
    Person.performance.avg()
)

print(f"Senior engineers (30+):")
print(f"  Avg salary: ${eng_stats['avg_salary']:,.2f}")
print(f"  Avg performance: {eng_stats['avg_performance']:.1f}")

# Filter THEN group THEN aggregate
senior_stats_by_dept = manager.filter(
    Person.job_title.contains(JobTitle("Senior"))
).group_by(Person.department).aggregate(
    Person.salary.avg(),
    Person.age.avg()
)

print("Senior staff by department:")
for dept, stats in senior_stats_by_dept.items():
    print(f"{dept}:")
    print(f"  Avg salary: ${stats['avg_salary']:,.2f}")
    print(f"  Avg age: {stats['avg_age']:.1f}")
```

## Query Chaining and Pagination

All query methods support chaining for complex queries:

### Build Queries Step by Step

```python
# Start with base query
query = manager.filter(Person.age.gt(Age(25)))

# Add more filters
query = query.filter(Person.department.eq(Department("Engineering")))

# Add pagination
query = query.limit(10).offset(20)

# Execute
results = query.execute()
```

### Chain in One Expression

```python
# Combine all operations
results = manager.filter(
    Person.age.gt(Age(25)),
    Person.department.eq(Department("Engineering"))
).limit(10).offset(20).execute()
```

### Pagination Methods

```python
# First page (10 items)
page1 = manager.filter(Person.active.eq(Active(True))).limit(10).execute()

# Second page (skip 10, take 10)
page2 = manager.filter(Person.active.eq(Active(True))).limit(10).offset(10).execute()

# Third page (skip 20, take 10)
page3 = manager.filter(Person.active.eq(Active(True))).limit(10).offset(20).execute()
```

### Sorting

Sort query results using Django-style field syntax:

```python
# Sort by single field (ascending)
results = manager.filter().order_by('name').execute()

# Descending order (prefix with '-')
results = manager.filter().order_by('-age').execute()

# Multiple fields (primary, secondary sort)
results = manager.filter().order_by('city', '-age').execute()
```

#### Available Sort Syntax

| Syntax         | Description                       | Example                     |
| -------------- | --------------------------------- | --------------------------- |
| `'field'`      | Ascending by field                | `order_by('age')`           |
| `'-field'`     | Descending by field               | `order_by('-age')`          |
| `'f1', 'f2'`   | Multiple fields                   | `order_by('city', '-age')`  |
| `'role__attr'` | Role-player attribute (Relations) | `order_by('employee__age')` |

#### Sorting Role-Player Attributes

For `RelationQuery`, you can sort by attributes of role players:

```python
# Sort employments by employee age
results = Employment.manager(db).filter().order_by('employee__age').execute()

# Sort by employer name descending
results = Employment.manager(db).filter().order_by('-employer__name').execute()

# Combined with role-player lookup filter
results = (
    Employment.manager(db)
    .filter(employee__age__gte=30)
    .order_by('-salary')
    .execute()
)
```

#### Sorting with Pagination

`order_by()` works with `limit()` and `offset()` for paginated results:

```python
# Page 1: First 10 results, sorted by age
page1 = manager.filter().order_by('age').limit(10).execute()

# Page 2: Next 10 results
page2 = manager.filter().order_by('age').limit(10).offset(10).execute()
```

> **Note**: When using `limit()` or `offset()` without `order_by()`, a default sort
> attribute is automatically selected to ensure stable pagination.

### Get First Result

```python
# Get first matching entity (returns Entity | None)
first = manager.filter(Person.age.gt(Age(30))).first()

if first:
    print(f"Found: {first.name}")
else:
    print("No match found")
```

### Count Without Fetching

```python
# Count matching entities without fetching data
count = manager.filter(Person.department.eq(Department("Sales"))).count()
print(f"Found {count} sales people")
```

## Backward Compatibility

The expression API coexists with the dictionary filter API:

### Dictionary Filters (Exact Match Only)

```python
# Old style - still works for exact matches
persons = manager.filter(age=30, status="active").execute()
```

### Expression Filters (Advanced)

```python
# New style - supports comparisons, ranges, string ops
persons = manager.filter(
    Person.age.gt(Age(30)),
    Person.status.eq(Status("active"))
).execute()
```

### Mixed Style

```python
# Both together (dict filters are exact match)
persons = manager.filter(
    Person.age.gt(Age(25)),  # Expression filter
    status="active"           # Dict filter (exact match)
).execute()
```

## Type Safety Guarantees

All expression operations are fully type-safe:

```python
# ✅ Type-safe: Age field has numeric methods
Person.age.gt(Age(30))
Person.age.avg()

# ✅ Type-safe: Name field has string methods
Person.name.contains(Name("Alice"))
Person.name.like(Name("A.*"))

# ❌ Type error: String field doesn't have numeric methods
Person.name.avg()  # Caught by type checker!

# ❌ Type error: Numeric field doesn't have string methods
Person.age.contains(Age(30))  # Caught by type checker!

# ✅ Type-safe: Expression returns correct type
expr: ComparisonExpr[Age] = Person.age.gt(Age(30))
str_expr: StringExpr[Name] = Person.name.contains(Name("Alice"))
agg_expr: AggregateExpr[Age] = Person.age.avg()
```

## Type-Safe Role Player Expressions

**New in v0.9.0**: Filter relations by role player attributes with full type safety.

### Basic Role Player Field Access

Access role player attributes through the relation class:

```python
from type_bridge import Relation, Role, Entity, TypeFlags, String, Integer, Flag, Key

class Name(String):
    pass

class Age(Integer):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None

class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)

class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

# Class-level access returns RoleRef for query building
Employment.employee       # Returns RoleRef with player_types=(Person,)
Employment.employee.age   # Returns RolePlayerNumericFieldRef
Employment.employee.name  # Returns RolePlayerStringFieldRef

# Instance-level access returns actual entity instances
emp = Employment(employee=person, employer=company)
emp.employee              # Returns Person instance
```

### Filtering by Role Player Attributes

Use comparison methods on role player field references:

```python
manager = Employment.manager(db)

# Greater than
results = manager.filter(Employment.employee.age.gt(Age(30))).execute()

# Less than or equal
results = manager.filter(Employment.employee.age.lte(Age(50))).execute()

# Equality
results = manager.filter(Employment.employee.age.eq(Age(25))).execute()

# Not equal
results = manager.filter(Employment.employee.age.neq(Age(30))).execute()
```

### String Operations on Role Player Attributes

String attributes support text search methods:

```python
# Contains substring
results = manager.filter(
    Employment.employer.name.contains(Name("Tech"))
).execute()

# Regex pattern (like)
results = manager.filter(
    Employment.employer.name.like(Name("^Tech.*"))
).execute()

# Regex pattern
results = manager.filter(
    Employment.employee.name.regex(Name("^A.*"))
).execute()
```

### Available Role Player Field Methods

**Numeric fields** (`RolePlayerNumericFieldRef`):

- `.gt(value)` - Greater than
- `.lt(value)` - Less than
- `.gte(value)` - Greater than or equal
- `.lte(value)` - Less than or equal
- `.eq(value)` - Equal to
- `.neq(value)` - Not equal to

**String fields** (`RolePlayerStringFieldRef`):

- `.contains(value)` - Substring match
- `.like(value)` - Regex pattern
- `.regex(value)` - Regex pattern (alias for like)

### Combining with Django-Style Filters

Mix type-safe expressions with Django-style keyword filters:

```python
# Type-safe expression + Django-style in same filter
results = manager.filter(
    Employment.employee.age.gt(Age(25)),
    employer__industry__eq="Technology"
).execute()

# Chained filters
results = (
    manager.filter(Employment.employee.age.gte(Age(25)))
    .filter(employer__industry__eq="Technology")
    .execute()
)
```

### Combined with Sorting and Pagination

Full query combining type-safe expressions, Django-style filters, sorting, and pagination:

```python
results = (
    Employment.manager(db)
    .filter(Employment.employee.age.gte(Age(25)), salary__gte=80000)
    .order_by("employee__age", "-salary")
    .limit(10)
    .offset(0)
    .execute()
)
```

### Multi-Player Roles (Role.multi)

For roles with multiple player types, access attributes from any player type:

```python
class Document(Entity):
    flags = TypeFlags(name="document")
    name: Name = Flag(Key)

class Email(Entity):
    flags = TypeFlags(name="email")
    subject: Subject = Flag(Key)
    sender: Sender

class Trace(Relation):
    flags = TypeFlags(name="trace")
    origin: Role[Document | Email] = Role.multi("origin", Document, Email)

# Access attributes from any player type
Trace.origin.name      # From Document
Trace.origin.subject   # From Email
Trace.origin.sender    # From Email

# dir() returns union of all player attributes
dir(Trace.origin)  # ['name', 'subject', 'sender', ...]
```

## Complete Query Example

```python
from type_bridge import Database, Entity, TypeFlags
from type_bridge.attribute import String, Integer, Double
from type_bridge.attribute.flags import Flag, Key

# Define schema
class Email(String):
    pass

class Age(Integer):
    pass

class Salary(Double):
    pass

class Department(String):
    pass

class JobTitle(String):
    pass

class Performance(Double):
    pass

class Employee(Entity):
    flags = TypeFlags(name="employee")
    email: Email = Flag(Key)
    age: Age
    salary: Salary
    department: Department
    job_title: JobTitle
    performance: Performance

# Connect
db = Database(address="localhost:1729", database="company")
db.connect()
manager = Employee.manager(db)

# Simple comparison
older = manager.filter(Employee.age.gt(Age(30))).execute()

# Range query
mid_age = manager.filter(
    Employee.age.gte(Age(25)),
    Employee.age.lt(Age(50))
).execute()

# String operations
gmail = manager.filter(Employee.email.contains(Email("@gmail.com"))).execute()
seniors = manager.filter(Employee.job_title.like(JobTitle("Senior.*"))).execute()

# Boolean logic: young high performers OR experienced employees
talent = manager.filter(
    Employee.age.lt(Age(30)).and_(
        Employee.salary.gt(Salary(80000.0))
    ).or_(
        Employee.age.gte(Age(45))
    )
).execute()

# Aggregation: average salary by department
dept_salaries = manager.group_by(Employee.department).aggregate(
    Employee.salary.avg(),
    Employee.age.avg()
)

for dept, stats in dept_salaries.items():
    print(f"{dept}: ${stats['avg_salary']:,.2f}, {stats['avg_age']:.1f} years")

# Pagination
page1 = manager.filter(
    Employee.department.eq(Department("Engineering"))
).limit(10).execute()

# First result
top_performer = manager.filter(
    Employee.performance.gte(Performance(95.0))
).first()

# Count
eng_count = manager.filter(
    Employee.department.eq(Department("Engineering"))
).count()
print(f"Engineers: {eng_count}")
```

## Best Practices

### 1. Use Field References for Advanced Queries

```python
# ✅ GOOD: Use field references for comparisons
manager.filter(Person.age.gt(Age(30)))

# ⚠️ LIMITED: Dict filters only support exact match
manager.filter(age=30)
```

### 2. Prefer Database Aggregations

```python
# ✅ GOOD: Aggregate on database
stats = manager.aggregate(Person.salary.avg())

# ❌ POOR: Fetch all and compute in Python
persons = manager.all()
avg = sum(p.salary.value for p in persons) / len(persons)
```

### 3. Use Pagination for Large Results

```python
# ✅ GOOD: Paginate large result sets
page = manager.filter(active=True).limit(100).offset(0).execute()

# ❌ POOR: Fetch everything
all_active = manager.filter(active=True).execute()  # Could be millions!
```

### 4. Use `first()` for Single Results

```python
# ✅ GOOD: Use first() when expecting one result
user = manager.filter(email="alice@example.com").first()

# ⚠️ VERBOSE: Get all and index
users = manager.filter(email="alice@example.com").execute()
user = users[0] if users else None
```

### 5. Combine Filters Efficiently

```python
# ✅ GOOD: Chain filters for readability
results = manager.filter(
    Person.age.gt(Age(25))
).filter(
    Person.department.eq(Department("Engineering"))
).execute()

# ✅ ALSO GOOD: All filters at once
results = manager.filter(
    Person.age.gt(Age(25)),
    Person.department.eq(Department("Engineering"))
).execute()
```

## Chainable Operations

**New in v0.6.0**: Expression-based filters can now be combined with chainable delete and update operations.

### Chainable Delete

Delete entities matching complex filter expressions:

```python
# Delete all persons over 65
count = manager.filter(Age.gt(Age(65))).delete()

# Delete with multiple expression filters
count = manager.filter(
    Age.lt(Age(18)),
    Status.eq(Status("inactive"))
).delete()

# Delete with range filters
count = manager.filter(
    Age.gte(Age(18)),
    Age.lt(Age(21))
).delete()
```

### Chainable Update with Functions

Update multiple entities using lambda or named functions:

```python
# Increment age using lambda
updated = manager.filter(Age.gt(Age(30))).update_with(
    lambda person: setattr(person, 'age', Age(person.age.value + 1))
)

# Complex updates with named function
def promote(person):
    person.status = Status("senior")
    person.salary = Salary(int(person.salary.value * 1.1))

promoted = manager.filter(Age.gte(Age(35))).update_with(promote)
```

**Benefits**:

- Works with all expression types (comparisons, strings, boolean logic)
- Single atomic transaction (all-or-nothing)
- Returns count (delete) or list of entities (update_with)
- Type-safe and validated at runtime

For detailed documentation, see [CRUD Operations - Chainable Delete](crud.md#chainable-delete) and [CRUD Operations - Bulk Update](crud.md#bulk-update-with-function).

## Arithmetic Expressions

TypeQL supports six infix arithmetic operators: `+`, `-`, `*`, `/`, `%`, `^`. TypeBridge provides `ArithmeticExpr` and helper functions to build these in queries:

```python
from type_bridge.expressions import add, sub, mul, div, mod, pow_

# Helper functions auto-normalize variable names (bare names get $ prefix)
expr = add("x", "y")          # ($x + $y)
expr = mul("price", 1.1)      # ($price * 1.1)
expr = mod("x", 2)            # ($x % 2)
expr = pow_("base", 3)        # ($base ^ 3)
```

Expressions can be nested via AST nodes for complex calculations:

```python
from type_bridge.query.ast import ArithmeticValue

# (($x + $y) * $z)
inner = ArithmeticValue(left="$x", operator="+", right="$y")
outer = ArithmeticValue(left=inner, operator="*", right="$z")
```

Each helper returns an `ArithmeticExpr` which implements the standard `Expression` interface (`to_ast()`, `to_value_ast()`, `to_typeql()`).

| Helper | Operator | Example             |
| ------ | -------- | ------------------- |
| `add`  | `+`      | `add("x", "y")`     |
| `sub`  | `-`      | `sub("x", "y")`     |
| `mul`  | `*`      | `mul("x", 2)`       |
| `div`  | `/`      | `div("total", "n")` |
| `mod`  | `%`      | `mod("x", 2)`       |
| `pow_` | `^`      | `pow_("base", 3)`   |

## See Also

- [CRUD Operations](crud.md) - Basic CRUD operations
- [Entities](entities.md) - Entity definition
- [Attributes](attributes.md) - Attribute types
- [Cardinality](cardinality.md) - Cardinality constraints
