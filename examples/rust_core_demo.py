"""
Demo of using the Rust-based type_bridge_core.
Note: This requires type_bridge_core to be built and installed.
"""

try:
    import type_bridge_core as core
    from type_bridge_core.ast import LiteralValue, EntityPattern, MatchClause
except ImportError:
    print("type_bridge_core not found. Build it with 'cd type-bridge-core && maturin develop'")
    # Mocking for demonstration if not installed
    class Mock:
        def __getattr__(self, name): return lambda *a, **kw: None
    core = Mock()
    LiteralValue = EntityPattern = MatchClause = lambda *a, **kw: None

def main():
    # 1. Create a validation engine
    engine = core.ValidationEngine()

    # 2. Construct a query using Rust-backed AST nodes
    name_val = LiteralValue("Alice", "string")
    person_pattern = EntityPattern(
        variable="$p",
        type_name="person",
        constraints=[core.ast.HasConstraint("name", name_val)]
    )
    
    match_clause = MatchClause([person_pattern])

    # 3. Validate the query (logic happens in Rust)
    is_valid = engine.validate(match_clause)
    print(f"Query is valid: {is_valid}")

    # 4. In the future, compile to TypeQL string in Rust
    # tql = engine.compile([match_clause])
    # print(f"Compiled TypeQL: {tql}")

if __name__ == "__main__":
    main()
