"""Integration tests for Decimal attribute CRUD operations."""

from decimal import Decimal as PyDecimal

import pytest

from type_bridge import Decimal, Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(51)
def test_decimal_insert(clean_db):
    """Test inserting entity with Decimal attribute."""

    class AccountName(String):
        pass

    class Balance(Decimal):
        pass

    class Account(Entity):
        flags = TypeFlags(name="account_dec")
        name: AccountName = Flag(Key)
        balance: Balance

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Account)
    schema_manager.sync_schema(force=True)

    manager = Account.manager(clean_db)

    # Insert account with Decimal
    account = Account(name=AccountName("Savings"), balance=Balance(PyDecimal("1234.56")))
    manager.insert(account)

    # Verify insertion
    results = manager.get(name="Savings")
    assert len(results) == 1
    assert results[0].balance.value == PyDecimal("1234.56")


@pytest.mark.integration
@pytest.mark.order(52)
def test_decimal_fetch(clean_db):
    """Test fetching entity by Decimal attribute."""

    class AccountName(String):
        pass

    class Balance(Decimal):
        pass

    class Account(Entity):
        flags = TypeFlags(name="account_dec_fetch")
        name: AccountName = Flag(Key)
        balance: Balance

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Account)
    schema_manager.sync_schema(force=True)

    manager = Account.manager(clean_db)

    # Insert accounts
    accounts = [
        Account(name=AccountName("Checking"), balance=Balance(PyDecimal("500.00"))),
        Account(name=AccountName("Investment"), balance=Balance(PyDecimal("10000.00"))),
    ]
    manager.insert_many(accounts)

    # Fetch by Decimal value
    results = manager.get(balance=PyDecimal("500.00"))
    assert len(results) == 1
    assert results[0].name.value == "Checking"


@pytest.mark.integration
@pytest.mark.order(53)
def test_decimal_update(clean_db):
    """Test updating Decimal attribute."""

    class AccountName(String):
        pass

    class Balance(Decimal):
        pass

    class Account(Entity):
        flags = TypeFlags(name="account_dec_update")
        name: AccountName = Flag(Key)
        balance: Balance

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Account)
    schema_manager.sync_schema(force=True)

    manager = Account.manager(clean_db)

    # Insert account
    account = Account(name=AccountName("Business"), balance=Balance(PyDecimal("5000.00")))
    manager.insert(account)

    # Fetch and update
    results = manager.get(name="Business")
    account_fetched = results[0]
    account_fetched.balance = Balance(PyDecimal("5500.00"))
    manager.update(account_fetched)

    # Verify update
    updated = manager.get(name="Business")
    assert updated[0].balance.value == PyDecimal("5500.00")


@pytest.mark.integration
@pytest.mark.order(54)
def test_decimal_delete(clean_db):
    """Test deleting entity with Decimal attribute."""

    class AccountName(String):
        pass

    class Balance(Decimal):
        pass

    class Account(Entity):
        flags = TypeFlags(name="account_dec_delete")
        name: AccountName = Flag(Key)
        balance: Balance

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Account)
    schema_manager.sync_schema(force=True)

    manager = Account.manager(clean_db)

    # Insert account
    account = Account(name=AccountName("Temp"), balance=Balance(PyDecimal("100.00")))
    manager.insert(account)

    # Delete by Decimal attribute using filter
    deleted_count = manager.filter(balance=PyDecimal("100.00")).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(name="Temp")
    assert len(results) == 0
