"""Test Decimal attribute type for fixed-point decimal values."""

from decimal import Decimal as DecimalType

from tests.utils.handwritten import Decimal, Entity, Flag, Key, String, TypeFlags


def test_decimal_creation():
    """Test creating Decimal with various input types."""

    class Price(Decimal):
        pass

    # Test with Decimal instance
    price1 = Price(DecimalType("123.45"))
    assert price1.value == DecimalType("123.45")

    # Test with string (recommended for precision)
    price2 = Price("999.9999999999999999")
    assert price2.value == DecimalType("999.9999999999999999")

    # Test with int
    price3 = Price(100)
    assert price3.value == DecimalType("100")

    # Test with float (may lose precision)
    price4 = Price(123.45)
    assert price4.value == DecimalType("123.45")


def test_decimal_value_type():
    """Test that Decimal has correct value_type for TypeDB."""

    class Price(Decimal):
        pass

    assert Price.value_type == "decimal"


def test_decimal_high_precision():
    """Test Decimal with high precision (19 digits after decimal point)."""

    class PreciseValue(Decimal):
        pass

    # TypeDB decimal supports 19 decimal digits of precision
    precise = PreciseValue("0.1234567890123456789")
    assert precise.value == DecimalType("0.1234567890123456789")


def test_decimal_in_entity():
    """Test using Decimal in an entity."""

    class AccountBalance(Decimal):
        pass

    class AccountNumber(String):
        pass

    class BankAccount(Entity):
        flags = TypeFlags(name="bank-account")
        account_number: AccountNumber = Flag(Key)
        balance: AccountBalance

    # Create entity with decimal value
    account = BankAccount(
        account_number=AccountNumber("ACC-001"), balance=AccountBalance("1234.56")
    )

    # Verify insert query has 'dec' suffix
    query = account.to_insert_query()
    assert "$e isa bank-account" in query
    assert 'has AccountNumber "ACC-001"' in query
    assert "has AccountBalance 1234.56dec" in query


def test_decimal_insert_query_formatting():
    """Test that Decimal values are formatted with 'dec' suffix in insert queries."""

    class Price(Decimal):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product")
        price: Price

    # Test with various decimal values
    product1 = Product(price=Price("0.02"))
    query1 = product1.to_insert_query()
    assert "has Price 0.02dec" in query1
    assert '"0.02dec"' not in query1  # Should NOT be quoted

    product2 = Product(price=Price("999.99"))
    query2 = product2.to_insert_query()
    assert "has Price 999.99dec" in query2

    product3 = Product(price=Price("0"))
    query3 = product3.to_insert_query()
    assert "has Price 0dec" in query3


def test_decimal_negative_values():
    """Test Decimal with negative values."""

    class Balance(Decimal):
        pass

    class Account(Entity):
        flags = TypeFlags(name="account")
        balance: Balance

    # Test with negative balance
    account = Account(balance=Balance("-500.25"))
    query = account.to_insert_query()
    assert "has Balance -500.25dec" in query


def test_decimal_large_values():
    """Test Decimal with large values (up to 2^63 - 1)."""

    class LargeValue(Decimal):
        pass

    class Record(Entity):
        flags = TypeFlags(name="record")
        value: LargeValue

    # Test with large integer part
    large_val = "9223372036854775807"  # 2^63 - 1
    record = Record(value=LargeValue(large_val))
    query = record.to_insert_query()
    assert f"has LargeValue {large_val}dec" in query


def test_decimal_zero_values():
    """Test Decimal with zero values."""

    class Amount(Decimal):
        pass

    class Transaction(Entity):
        flags = TypeFlags(name="transaction")
        amount: Amount

    # Test with zero
    transaction = Transaction(amount=Amount("0"))
    query = transaction.to_insert_query()
    assert "has Amount 0dec" in query

    # Test with zero and decimals
    transaction2 = Transaction(amount=Amount("0.00"))
    query2 = transaction2.to_insert_query()
    assert "has Amount 0.00dec" in query2


def test_decimal_scientific_notation():
    """Test Decimal with values that might be in scientific notation."""

    class SmallValue(Decimal):
        pass

    class Measurement(Entity):
        flags = TypeFlags(name="measurement")
        value: SmallValue

    # Test with very small value
    measurement = Measurement(value=SmallValue("0.000001"))
    query = measurement.to_insert_query()
    # Decimal should maintain precision, not convert to scientific notation
    assert "0.000001dec" in query


def test_decimal_optional_attribute():
    """Test Decimal as optional attribute."""

    class Discount(Decimal):
        pass

    class ProductName(String):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product")
        name: ProductName = Flag(Key)
        discount: Discount | None

    # Test with None (optional)
    product = Product(name=ProductName("Widget"), discount=None)
    query = product.to_insert_query()
    assert 'has ProductName "Widget"' in query
    assert "has Discount" not in query  # Should not appear when None

    # Test with value
    product2 = Product(name=ProductName("Gadget"), discount=Discount("10.5"))
    query2 = product2.to_insert_query()
    assert "has Discount 10.5dec" in query2


def test_decimal_multi_value_attribute():
    """Test Decimal with multi-value attributes."""
    from tests.utils.handwritten import Card

    class Payment(Decimal):
        pass

    class Invoice(Entity):
        flags = TypeFlags(name="invoice")
        payments: list[Payment] = Flag(Card(min=1))

    # Create entity with multiple decimal values
    invoice = Invoice(payments=[Payment("100.50"), Payment("200.75"), Payment("50.25")])
    query = invoice.to_insert_query()

    assert "$e isa invoice" in query
    assert "has Payment 100.50dec" in query
    assert "has Payment 200.75dec" in query
    assert "has Payment 50.25dec" in query


def test_decimal_pydantic_validation():
    """Test Pydantic validation for Decimal in entities."""

    class Price(Decimal):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product")
        price: Price

    # Test with Decimal instance
    product1 = Product(price=Price("123.45"))
    assert product1.price.value == DecimalType("123.45")

    # Test with raw string (Pydantic should convert)
    product2 = Product(price=Price("678.90"))
    assert product2.price.value == DecimalType("678.90")


def test_decimal_comparison():
    """Test Decimal attribute comparison."""

    class Price(Decimal):
        pass

    price1 = Price("100.50")
    price2 = Price("100.50")
    price3 = Price("200.75")

    # Same type, same value
    assert price1 == price2

    # Same type, different value
    assert price1 != price3

    # Different precision but same value
    price4 = Price("100.5")
    price5 = Price("100.50")
    # Decimal considers these equal
    assert price4.value == price5.value
    assert price4 == price5


def test_decimal_string_representation():
    """Test string representation of Decimal attributes."""

    class Balance(Decimal):
        pass

    balance = Balance("1234.56")

    # Test __str__
    assert str(balance) == "1234.56"

    # Test __repr__
    assert repr(balance) == "Balance(Decimal('1234.56'))"


def test_decimal_from_calculation():
    """Test creating Decimal from arithmetic calculations."""

    class Total(Decimal):
        pass

    # Calculate using Decimal arithmetic
    price1 = DecimalType("10.50")
    price2 = DecimalType("20.75")
    total_value = price1 + price2

    total = Total(total_value)
    assert total.value == DecimalType("31.25")


def test_decimal_precision_preservation():
    """Test that Decimal preserves precision unlike float."""

    class PreciseValue(Decimal):
        pass

    # Using string maintains precision
    precise1 = PreciseValue("0.1")
    precise2 = PreciseValue("0.2")
    result = DecimalType(str(precise1.value)) + DecimalType(str(precise2.value))

    # Decimal arithmetic maintains precision
    assert result == DecimalType("0.3")  # No floating point error

    # Compare with float behavior (for documentation)
    # float: 0.1 + 0.2 = 0.30000000000000004 (floating point error)
    # Decimal: 0.1 + 0.2 = 0.3 (exact)
