"""Integration tests for Double attribute CRUD operations."""

import pytest

from type_bridge import Double, Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(35)
def test_double_insert(clean_db):
    """Test inserting entity with Double attribute."""

    class ProductName(String):
        pass

    class Price(Double):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product_dbl")
        name: ProductName = Flag(Key)
        price: Price

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Product)
    schema_manager.sync_schema(force=True)

    manager = Product.manager(clean_db)

    # Insert product with Double
    product = Product(name=ProductName("Laptop"), price=Price(999.99))
    manager.insert(product)

    # Verify insertion
    results = manager.get(name="Laptop")
    assert len(results) == 1
    assert abs(results[0].price.value - 999.99) < 0.01


@pytest.mark.integration
@pytest.mark.order(36)
def test_double_fetch(clean_db):
    """Test fetching entity by Double attribute."""

    class ProductName(String):
        pass

    class Price(Double):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product_dbl_fetch")
        name: ProductName = Flag(Key)
        price: Price

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Product)
    schema_manager.sync_schema(force=True)

    manager = Product.manager(clean_db)

    # Insert products
    products = [
        Product(name=ProductName("Mouse"), price=Price(29.99)),
        Product(name=ProductName("Keyboard"), price=Price(79.99)),
    ]
    manager.insert_many(products)

    # Fetch by Double value
    cheap_products = manager.get(price=29.99)
    assert len(cheap_products) == 1
    assert cheap_products[0].name.value == "Mouse"


@pytest.mark.integration
@pytest.mark.order(37)
def test_double_update(clean_db):
    """Test updating Double attribute."""

    class ProductName(String):
        pass

    class Price(Double):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product_dbl_update")
        name: ProductName = Flag(Key)
        price: Price

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Product)
    schema_manager.sync_schema(force=True)

    manager = Product.manager(clean_db)

    # Insert product
    product = Product(name=ProductName("Monitor"), price=Price(299.99))
    manager.insert(product)

    # Fetch and update
    results = manager.get(name="Monitor")
    product_fetched = results[0]
    product_fetched.price = Price(249.99)
    manager.update(product_fetched)

    # Verify update
    updated = manager.get(name="Monitor")
    assert abs(updated[0].price.value - 249.99) < 0.01


@pytest.mark.integration
@pytest.mark.order(38)
def test_double_delete(clean_db):
    """Test deleting entity with Double attribute."""

    class ProductName(String):
        pass

    class Price(Double):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product_dbl_delete")
        name: ProductName = Flag(Key)
        price: Price

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Product)
    schema_manager.sync_schema(force=True)

    manager = Product.manager(clean_db)

    # Insert product
    product = Product(name=ProductName("Speaker"), price=Price(149.99))
    manager.insert(product)

    # Delete by Double attribute using filter
    deleted_count = manager.filter(price=149.99).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(name="Speaker")
    assert len(results) == 0
