# Validation

Complete reference for type validation, Pydantic integration, and type safety features in TypeBridge.

## Overview

TypeBridge is built on **Pydantic v2**, providing powerful validation, serialization, and type safety features. All entities and relations are Pydantic models with automatic type validation, JSON serialization, and field validation.

## Pydantic Integration

### Automatic Type Validation

All attribute values are automatically validated to the correct type:

```python
from type_bridge import Entity, TypeFlags, String, Integer

class Name(String):
    pass

class Age(Integer):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name
    age: Age

# ✅ Valid: Correct types
person = Person(name=Name("Alice"), age=Age(30))

# ❌ Invalid: Type mismatch (Pydantic raises ValidationError)
try:
    person = Person(name=Name("Alice"), age="thirty")  # String instead of Age
except ValidationError as e:
    print(e)
    # Output: validation error showing expected Age, got str
```

### Validation on Assignment

Field assignments are automatically validated:

```python
# Create valid person
person = Person(name=Name("Alice"), age=Age(30))

# ✅ Valid assignment
person.age = Age(31)

# ❌ Invalid assignment (Pydantic raises ValidationError)
try:
    person.age = "thirty-one"  # Wrong type
except ValidationError as e:
    print(e)
```

## JSON Serialization

### Serialize to JSON

Convert entities to JSON using Pydantic's serialization:

```python
from type_bridge import Entity, TypeFlags, String, Integer, Boolean

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name
    age: Age
    is_active: IsActive

person = Person(
    name=Name("Alice"),
    age=Age(30),
    is_active=IsActive(True)
)

# Serialize to JSON string
json_data = person.model_dump_json()
print(json_data)
# Output: {"name":"Alice","age":30,"is_active":true}

# Serialize to dict
dict_data = person.model_dump()
print(dict_data)
# Output: {'name': Name('Alice'), 'age': Age(30), 'is_active': IsActive(True)}
```

### Deserialize from JSON

Create entities from JSON data:

```python
# Deserialize from JSON string
json_str = '{"name":"Bob","age":25,"is_active":false}'
bob = Person.model_validate_json(json_str)

print(bob.name)       # Name('Bob')
print(bob.age)        # Age(25)
print(bob.is_active)  # IsActive(False)

# Deserialize from dict
data = {"name": "Charlie", "age": 35, "is_active": True}
charlie = Person.model_validate(data)
```

### Serialization Options

Control serialization behavior:

```python
# Exclude unset fields
person = Person(name=Name("Alice"), age=Age(30))
json_data = person.model_dump_json(exclude_unset=True)

# Exclude specific fields
json_data = person.model_dump_json(exclude={"age"})

# Include only specific fields
json_data = person.model_dump_json(include={"name", "age"})

# Use by_alias for field name mapping
json_data = person.model_dump_json(by_alias=True)
```

## Model Copying

Create modified copies of entities:

```python
# Create original
alice = Person(name=Name("Alice"), age=Age(30))

# Create copy with modifications
alice_older = alice.model_copy(update={"age": Age(31)})

print(alice.age)        # Age(30) - original unchanged
print(alice_older.age)  # Age(31) - copy modified

# Deep copy
alice_deep = alice.model_copy(deep=True)
```

## Type Coercion

Pydantic automatically coerces compatible types:

```python
from type_bridge import String, Integer

class Name(String):
    pass

class Age(Integer):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name
    age: Age

# Direct string/int coercion
person = Person(name="Alice", age=30)  # Automatically wraps in Name/Age

print(type(person.name))  # <class 'Name'>
print(type(person.age))   # <class 'Age'>
```

## Literal Types for Type Safety

TypeBridge supports Python's `Literal` types for enum-like values with type-checker hints:

### Basic Literal Usage

```python
from typing import Literal
from type_bridge import Entity, TypeFlags, String, Integer

class Status(String):
    pass

class Priority(Integer):
    pass

class Task(Entity):
    flags = TypeFlags(name="task")

    # Type checker sees Literal and provides autocomplete/warnings
    status: Literal["pending", "active", "completed"] | Status
    priority: Literal[1, 2, 3, 4, 5] | Priority

# ✅ Valid literal values - IDE autocompletes
task1 = Task(status="pending", priority=1)
task2 = Task(status="active", priority=3)
task3 = Task(status="completed", priority=5)

# ⚠️ Type checker warns about invalid literals
task4 = Task(status="invalid", priority=10)  # Type warning in IDE

# ✅ Runtime accepts any valid type (Pydantic flexibility)
task5 = Task(status="custom_status", priority=999)  # Works at runtime
```

### Literal Type Benefits

1. **Type-checker safety**: IDEs and type checkers provide autocomplete and warnings
2. **Runtime flexibility**: Pydantic accepts any value matching the Attribute type
3. **Self-documenting**: Common values visible in type hints
4. **No restrictions**: Not enforced at runtime, allowing custom values

### Use Cases for Literals

```python
# Status fields with common values
class Status(String):
    pass

class Order(Entity):
    status: Literal["draft", "pending", "confirmed", "shipped", "delivered"] | Status

# Priority levels
class Priority(Integer):
    pass

class Issue(Entity):
    priority: Literal[1, 2, 3, 4, 5] | Priority

# Boolean flags with semantic meaning
class YesNo(String):
    pass

class Survey(Entity):
    response: Literal["yes", "no", "maybe"] | YesNo
```

## Model Configuration

Entities and Relations are configured with Pydantic settings:

```python
class Entity:
    model_config = {
        "arbitrary_types_allowed": True,    # Allow Attribute subclasses
        "validate_assignment": True,        # Validate field assignments
        "extra": "allow",                   # Allow extra fields
        "ignored_types": (TypeFlags, Role), # Ignore TypeBridge types
    }
```

### Configuration Options

- **`arbitrary_types_allowed=True`**: Allows custom Attribute subclass types
- **`validate_assignment=True`**: Validates values when assigning to fields
- **`extra="allow"`**: Allows extra fields beyond those defined
- **`ignored_types`**: TypeBridge-specific types ignored during validation

## Validation Errors

Pydantic raises `ValidationError` with detailed information:

```python
from pydantic import ValidationError

try:
    # Invalid: Wrong type
    person = Person(name=Name("Alice"), age="thirty")
except ValidationError as e:
    print(e.json())
    # Output: Detailed JSON error with field, type, and message

try:
    # Invalid: Missing required field
    person = Person(name=Name("Alice"))  # age is required
except ValidationError as e:
    print(e.errors())
    # Output: List of error dicts with field and error info
```

### Error Information

```python
try:
    person = Person(name=123, age="thirty")
except ValidationError as e:
    for error in e.errors():
        print(f"Field: {error['loc']}")
        print(f"Type: {error['type']}")
        print(f"Message: {error['msg']}")
```

## Custom Validators

Add custom validation logic with Pydantic validators:

```python
from pydantic import field_validator

class Age(Integer):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name
    age: Age

    @field_validator('age')
    @classmethod
    def validate_age(cls, v: Age) -> Age:
        if v.value < 0:
            raise ValueError("Age cannot be negative")
        if v.value > 150:
            raise ValueError("Age cannot exceed 150")
        return v

# ✅ Valid age
person = Person(name=Name("Alice"), age=Age(30))

# ❌ Invalid: Negative age
try:
    person = Person(name=Name("Bob"), age=Age(-5))
except ValidationError as e:
    print(e)  # "Age cannot be negative"

# ❌ Invalid: Age too high
try:
    person = Person(name=Name("Charlie"), age=Age(200))
except ValidationError as e:
    print(e)  # "Age cannot exceed 150"
```

## Reserved Word Validation

TypeBridge validates that type names don't conflict with TypeDB/TypeQL reserved words:

```python
from type_bridge.schema import SchemaValidationError

# ❌ WRONG: Using reserved words
class Type(Entity):  # Error: 'type' is reserved
    pass

class Match(Entity):  # Error: 'match' is reserved
    pass

class Attribute(Entity):  # Error: 'attribute' is reserved
    pass

# ✅ CORRECT: Use different names
class ContentType(Entity):
    pass

class MatchResult(Entity):
    pass

class CustomAttribute(Entity):
    pass
```

**Reserved words include**: `entity`, `relation`, `attribute`, `match`, `insert`, `delete`, `define`, `type`, `sub`, `owns`, `plays`, `relates`, `isa`, etc.

## Type Safety with Type Checkers

TypeBridge achieves 0 type errors with Pyright:

### Type-Safe Field Access

```python
# Type checker understands attribute types
person = Person(name=Name("Alice"), age=Age(30))

# ✅ Type-safe: name is Name
name: Name = person.name

# ✅ Type-safe: age is Age
age: Age = person.age

# ❌ Type error: Cannot assign wrong type
person.age = Name("thirty")  # Type checker error!
```

### Type-Safe Manager Operations

```python
# Type checker understands generic manager
person_manager = Person.manager(db)

# ✅ Type-safe: insert accepts Person
person_manager.insert(person)

# ✅ Type-safe: get returns list[Person]
persons: list[Person] = person_manager.all()

# ❌ Type error: Cannot insert wrong type
company = Company(name=Name("TechCorp"))
person_manager.insert(company)  # Type checker error!
```

### Type-Safe Query Expressions

```python
# ✅ Type-safe: Numeric field has numeric methods
Person.age.gt(Age(30))
Person.age.avg()

# ✅ Type-safe: String field has string methods
Person.name.contains(Name("Alice"))
Person.name.like(Name("A.*"))

# ❌ Type error: String field doesn't have numeric methods
Person.name.avg()  # Type checker error!

# ❌ Type error: Numeric field doesn't have string methods
Person.age.contains(Age(30))  # Type checker error!
```

## Complete Validation Example

```python
from typing import Literal
from pydantic import field_validator, ValidationError
from type_bridge import (
    Entity, TypeFlags,
    String, Integer, Boolean,
    Flag, Key, Unique
)

# Define attribute types
class UserID(String):
    pass

class Username(String):
    pass

class Email(String):
    pass

class Age(Integer):
    pass

class Status(String):
    pass

class IsVerified(Boolean):
    pass

# Define entity with validation
class User(Entity):
    flags = TypeFlags(name="user")

    user_id: UserID = Flag(Key)
    username: Username
    email: Email = Flag(Unique)
    age: Age
    status: Literal["active", "inactive", "pending"] | Status
    is_verified: IsVerified

    @field_validator('age')
    @classmethod
    def validate_age(cls, v: Age) -> Age:
        if v.value < 13:
            raise ValueError("User must be at least 13 years old")
        if v.value > 120:
            raise ValueError("Invalid age")
        return v

    @field_validator('username')
    @classmethod
    def validate_username(cls, v: Username) -> Username:
        if len(v.value) < 3:
            raise ValueError("Username must be at least 3 characters")
        if not v.value.isalnum():
            raise ValueError("Username must be alphanumeric")
        return v

    @field_validator('email')
    @classmethod
    def validate_email(cls, v: Email) -> Email:
        if '@' not in v.value:
            raise ValueError("Invalid email address")
        return v

# ✅ Valid user
try:
    user = User(
        user_id=UserID("u123"),
        username=Username("alice"),
        email=Email("alice@example.com"),
        age=Age(30),
        status="active",  # Literal type - IDE autocompletes
        is_verified=IsVerified(True)
    )
    print("✅ User created successfully")
except ValidationError as e:
    print(f"❌ Validation error: {e}")

# ❌ Invalid: Age too young
try:
    user = User(
        user_id=UserID("u456"),
        username=Username("kid"),
        email=Email("kid@example.com"),
        age=Age(10),  # Under 13
        status="active",
        is_verified=IsVerified(False)
    )
except ValidationError as e:
    print(f"❌ Validation error: {e}")

# ❌ Invalid: Username too short
try:
    user = User(
        user_id=UserID("u789"),
        username=Username("ab"),  # Less than 3 chars
        email=Email("user@example.com"),
        age=Age(25),
        status="active",
        is_verified=IsVerified(True)
    )
except ValidationError as e:
    print(f"❌ Validation error: {e}")

# Serialize to JSON
json_data = user.model_dump_json()
print(f"JSON: {json_data}")

# Deserialize from JSON
user_copy = User.model_validate_json(json_data)
print(f"Deserialized: {user_copy.username}")
```

## Best Practices

### 1. Use Literal Types for Common Values

```python
# ✅ GOOD: Literal provides IDE hints
status: Literal["draft", "published", "archived"] | Status

# ⚠️ LESS HELPFUL: No IDE hints
status: Status
```

### 2. Add Custom Validators for Business Logic

```python
# ✅ GOOD: Validate business rules
@field_validator('age')
@classmethod
def validate_age(cls, v: Age) -> Age:
    if v.value < 0:
        raise ValueError("Age cannot be negative")
    return v

# ❌ POOR: No validation
age: Age
```

### 3. Handle ValidationError Gracefully

```python
# ✅ GOOD: Catch and handle validation errors
try:
    person = Person(name=Name("Alice"), age=Age(150))
except ValidationError as e:
    print(f"Invalid data: {e.errors()}")
    # Handle error appropriately

# ❌ POOR: Let exceptions propagate
person = Person(name=Name("Alice"), age=Age(150))  # May crash
```

### 4. Use Type Hints for Type Safety

```python
# ✅ GOOD: Type hints enable type checking
persons: list[Person] = person_manager.all()

# ⚠️ LESS SAFE: No type hints
persons = person_manager.all()
```

### 5. Leverage JSON Serialization

```python
# ✅ GOOD: Use Pydantic serialization
json_data = person.model_dump_json()

# ❌ POOR: Manual serialization
import json
json_data = json.dumps({"name": person.name.value, "age": person.age.value})
```

## See Also

- [Attributes](attributes.md) - Attribute types and validation
- [Entities](entities.md) - Entity definition
- [Relations](relations.md) - Relation definition
- [Schema Management](schema.md) - Schema validation rules
