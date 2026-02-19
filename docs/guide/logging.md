# Logging Configuration

TypeBridge uses Python's standard `logging` module to provide comprehensive logging throughout the library. As a library, TypeBridge does not configure logging by default, giving you full control over logging configuration in your application.

## Quick Start

```python
import logging

# Basic setup - enable all TypeBridge logging
logging.basicConfig(level=logging.DEBUG)
logging.getLogger("type_bridge").setLevel(logging.DEBUG)
```

## Logger Hierarchy

TypeBridge loggers are organized hierarchically using the module structure:

```
type_bridge                              # Root - enable to see all logs
├── type_bridge.session                  # Database connections, transactions
├── type_bridge.schema                   # Schema management
│   ├── type_bridge.schema.manager       # SchemaManager operations
│   └── type_bridge.schema.migration     # Migration execution
├── type_bridge.crud                     # All CRUD operations
│   ├── type_bridge.crud.entity          # Entity operations
│   │   ├── type_bridge.crud.entity.manager
│   │   ├── type_bridge.crud.entity.query
│   │   └── type_bridge.crud.entity.group_by
│   └── type_bridge.crud.relation        # Relation operations
│       ├── type_bridge.crud.relation.manager
│       ├── type_bridge.crud.relation.query
│       └── type_bridge.crud.relation.group_by
├── type_bridge.query                    # Query builder
├── type_bridge.generator                # Code generator (TQL → Python)
│   ├── type_bridge.generator.parser
│   └── type_bridge.generator.render.*
├── type_bridge.models                   # Model definitions
│   ├── type_bridge.models.entity
│   └── type_bridge.models.relation
└── type_bridge.validation               # Name validation
```

## Configuration Examples

### Enable All TypeBridge Logging

```python
import logging

logging.basicConfig(
    level=logging.DEBUG,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logging.getLogger("type_bridge").setLevel(logging.DEBUG)
```

### Enable Only CRUD Logging

```python
import logging

logging.basicConfig(level=logging.INFO)

# Enable debug logging only for CRUD operations
logging.getLogger("type_bridge.crud").setLevel(logging.DEBUG)
```

### Enable Connection/Session Logging

```python
import logging

logging.basicConfig(level=logging.INFO)

# See connection lifecycle and query execution
logging.getLogger("type_bridge.session").setLevel(logging.DEBUG)
```

### Production Configuration

```python
import logging

# In production, you might want INFO level with file output
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    handlers=[
        logging.FileHandler('app.log'),
        logging.StreamHandler()
    ]
)

# TypeBridge at INFO level - significant events only
logging.getLogger("type_bridge").setLevel(logging.INFO)
```

### Integration with structlog

If your application uses `structlog`, TypeBridge logs integrate seamlessly since it uses stdlib logging:

```python
import logging
import structlog

structlog.configure(
    processors=[
        structlog.stdlib.filter_by_level,
        structlog.stdlib.add_logger_name,
        structlog.stdlib.add_log_level,
        structlog.stdlib.PositionalArgumentsFormatter(),
        structlog.processors.TimeStamper(fmt="iso"),
        structlog.processors.StackInfoRenderer(),
        structlog.processors.format_exc_info,
        structlog.processors.UnicodeDecoder(),
        structlog.stdlib.ProcessorFormatter.wrap_for_formatter,
    ],
    wrapper_class=structlog.stdlib.BoundLogger,
    context_class=dict,
    logger_factory=structlog.stdlib.LoggerFactory(),
    cache_logger_on_first_use=True,
)

# TypeBridge logs will flow through structlog's processors
logging.getLogger("type_bridge").setLevel(logging.DEBUG)
```

## Log Levels

TypeBridge uses log levels consistently:

| Level   | When Used                                          | Examples                                        |
|---------|----------------------------------------------------|-------------------------------------------------|
| DEBUG   | Detailed operations, query text, internal state    | `Executing: match $e isa person; fetch...`      |
| INFO    | Significant events, operation completion           | `Connected to TypeDB at localhost:1729`         |
| WARNING | Recoverable issues, validation concerns            | `Reserved word 'entity' used as type name`      |
| ERROR   | Failures requiring attention                       | `Failed to connect to TypeDB: Connection refused`|

## What Gets Logged

### Session/Connection Logs (type_bridge.session)

- DEBUG: Connection parameters, query text
- INFO: Successful connection, database creation/deletion
- ERROR: Connection failures, transaction errors

```
DEBUG - Connecting to TypeDB at localhost:1729 (database: mydb)
INFO - Connected to TypeDB at localhost:1729
DEBUG - Executing query: match $e isa person; fetch { $e.* };
INFO - Transaction committed
```

### CRUD Logs (type_bridge.crud.*)

- DEBUG: Generated TypeQL, filter details, query parameters
- INFO: Operation completion with counts

```
DEBUG - EntityManager.insert: Person
DEBUG - Generated insert query: $e isa person, has name "Alice"
INFO - Entity inserted: Person
DEBUG - EntityQuery.execute: Person, filters={'name': Name('Alice')}
INFO - EntityQuery executed: 1 entities returned
```

### Schema Logs (type_bridge.schema.*)

- DEBUG: Model registration, schema comparison details
- INFO: Schema sync start/complete
- WARNING: Schema conflicts detected

```
DEBUG - Registering model: Person
INFO - Syncing schema (force=False)
DEBUG - Collecting schema info for registered models
INFO - Schema sync complete: 3 types defined
```

### Generator Logs (type_bridge.generator.*)

- DEBUG: Parsing steps, inheritance resolution
- INFO: Generation summary

```
DEBUG - Starting TQL schema parsing
DEBUG - Found 15 statements to parse
DEBUG - Parsed entity: person
INFO - Schema parsed: 5 attributes, 3 entities, 2 relations
INFO - Rendered 5 attribute classes
```

### Validation Logs (type_bridge.validation)

- DEBUG: Validation checks
- WARNING: Validation failures with suggestions

```
DEBUG - Validating entity name: person
WARNING - Reserved word used as entity name: entity
```

## Filtering Noisy Logs

If TypeBridge logging is too verbose, you can filter specific modules:

```python
import logging

# Enable most logging but silence query debug
logging.getLogger("type_bridge").setLevel(logging.DEBUG)
logging.getLogger("type_bridge.query").setLevel(logging.INFO)  # Less verbose
```

## Testing with Logs

### pytest with Logging

```bash
# Show all logs during tests
uv run pytest --log-cli-level=DEBUG

# Show logs only for specific TypeBridge modules
uv run pytest --log-cli-level=DEBUG --log-cli-format="%(name)s - %(message)s"

# Capture logs to file during tests
uv run pytest --log-file=test.log --log-file-level=DEBUG
```

### In Test Code

```python
def test_with_logging(caplog):
    import logging

    with caplog.at_level(logging.DEBUG, logger="type_bridge"):
        # Your test code
        person = Person(name=Name("Alice"))
        Person.manager(db).insert(person)

    # Assert on log messages
    assert "Entity inserted" in caplog.text
```

## Troubleshooting

### No Logs Appearing

1. Ensure you've configured `logging.basicConfig()` before any TypeBridge imports
2. Check that you've set the log level on the correct logger
3. Verify your handler is configured to output at the appropriate level

### Too Many Logs

1. Set the root TypeBridge logger to INFO or WARNING
2. Enable DEBUG only for specific modules you're investigating
3. Use log level filtering in your handler

```python
# Only show WARNING and above
logging.getLogger("type_bridge").setLevel(logging.WARNING)

# But show DEBUG for the module you're investigating
logging.getLogger("type_bridge.crud.entity.manager").setLevel(logging.DEBUG)
```

## Best Practices

1. **Don't configure logging in library code** - TypeBridge never calls `basicConfig()` or adds handlers
2. **Use hierarchical filtering** - Enable `type_bridge.crud` rather than each sub-module individually
3. **Use DEBUG sparingly in production** - DEBUG logs include full query text which may be verbose
4. **Log sensitive data awareness** - TypeBridge logs query text but not connection credentials
