"""Integration tests for common real-world type combinations."""

from datetime import UTC, date, datetime, timedelta
from decimal import Decimal as PyDecimal

import pytest

from type_bridge import (
    Boolean,
    Date,
    DateTime,
    DateTimeTZ,
    Decimal,
    Double,
    Duration,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(82)
def test_person_entity_string_integer_date(clean_db):
    """Test Person entity with String, Integer, and Date types."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class BirthDate(Date):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_combo")
        name: Name = Flag(Key)
        age: Age
        birth_date: BirthDate

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person
    person = Person(name=Name("Alice"), age=Age(30), birth_date=BirthDate(date(1994, 3, 15)))
    manager.insert(person)

    # Fetch and verify
    results = manager.get(name="Alice")
    assert len(results) == 1
    assert results[0].age.value == 30
    assert results[0].birth_date.value == date(1994, 3, 15)


@pytest.mark.integration
@pytest.mark.order(83)
def test_product_entity_string_decimal_integer_boolean(clean_db):
    """Test Product entity with String, Decimal, Integer, and Boolean types."""

    class ProductName(String):
        pass

    class Price(Decimal):
        pass

    class Stock(Integer):
        pass

    class InStock(Boolean):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product_combo")
        name: ProductName = Flag(Key)
        price: Price
        stock: Stock
        in_stock: InStock

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Product)
    schema_manager.sync_schema(force=True)

    manager = Product.manager(clean_db)

    # Insert product
    product = Product(
        name=ProductName("Laptop"),
        price=Price(PyDecimal("999.99")),
        stock=Stock(50),
        in_stock=InStock(True),
    )
    manager.insert(product)

    # Fetch and verify
    results = manager.get(name="Laptop")
    assert len(results) == 1
    assert results[0].price.value == PyDecimal("999.99")
    assert results[0].stock.value == 50
    assert results[0].in_stock.value is True


@pytest.mark.integration
@pytest.mark.order(84)
def test_event_entity_string_datetime_duration(clean_db):
    """Test Event entity with String, DateTime, and Duration types."""

    class EventName(String):
        pass

    class StartTime(DateTime):
        pass

    class EventDuration(Duration):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_combo")
        name: EventName = Flag(Key)
        start_time: StartTime
        duration: EventDuration

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert event
    event = Event(
        name=EventName("Conference"),
        start_time=StartTime(datetime(2024, 6, 1, 9, 0, 0)),
        duration=EventDuration(timedelta(hours=8)),
    )
    manager.insert(event)

    # Fetch and verify
    results = manager.get(name="Conference")
    assert len(results) == 1
    assert results[0].start_time.value == datetime(2024, 6, 1, 9, 0, 0)
    assert results[0].duration.value == timedelta(hours=8)


@pytest.mark.integration
@pytest.mark.order(85)
def test_measurement_entity_string_double_datetimetz(clean_db):
    """Test Measurement entity with String, Double, and DateTimeTZ types."""

    class SensorName(String):
        pass

    class Temperature(Double):
        pass

    class MeasuredAt(DateTimeTZ):
        pass

    class Measurement(Entity):
        flags = TypeFlags(name="measurement_combo")
        sensor: SensorName = Flag(Key)
        temperature: Temperature
        measured_at: MeasuredAt

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Measurement)
    schema_manager.sync_schema(force=True)

    manager = Measurement.manager(clean_db)

    # Insert measurement
    measurement = Measurement(
        sensor=SensorName("TEMP-01"),
        temperature=Temperature(23.5),
        measured_at=MeasuredAt(datetime(2024, 1, 15, 10, 30, 0, tzinfo=UTC)),
    )
    manager.insert(measurement)

    # Fetch and verify
    results = manager.get(sensor="TEMP-01")
    assert len(results) == 1
    assert abs(results[0].temperature.value - 23.5) < 0.01
    assert results[0].measured_at.value == datetime(2024, 1, 15, 10, 30, 0, tzinfo=UTC)


@pytest.mark.integration
@pytest.mark.order(86)
def test_account_entity_string_boolean_datetime(clean_db):
    """Test Account entity with String, Boolean, and DateTime types."""

    class Username(String):
        pass

    class IsVerified(Boolean):
        pass

    class CreatedAt(DateTime):
        pass

    class Account(Entity):
        flags = TypeFlags(name="account_combo")
        username: Username = Flag(Key)
        is_verified: IsVerified
        created_at: CreatedAt

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Account)
    schema_manager.sync_schema(force=True)

    manager = Account.manager(clean_db)

    # Insert account
    account = Account(
        username=Username("alice123"),
        is_verified=IsVerified(True),
        created_at=CreatedAt(datetime(2024, 1, 1, 10, 0, 0)),
    )
    manager.insert(account)

    # Fetch and verify
    results = manager.get(username="alice123")
    assert len(results) == 1
    assert results[0].is_verified.value is True
    assert results[0].created_at.value == datetime(2024, 1, 1, 10, 0, 0)


@pytest.mark.integration
@pytest.mark.order(87)
def test_order_entity_integer_decimal_date_string(clean_db):
    """Test Order entity with Integer, Decimal, Date, and String types."""

    class OrderNumber(String):
        pass

    class Quantity(Integer):
        pass

    class TotalPrice(Decimal):
        pass

    class OrderDate(Date):
        pass

    class Order(Entity):
        flags = TypeFlags(name="order_combo")
        order_number: OrderNumber = Flag(Key)
        quantity: Quantity
        total_price: TotalPrice
        order_date: OrderDate

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Order)
    schema_manager.sync_schema(force=True)

    manager = Order.manager(clean_db)

    # Insert order
    order = Order(
        order_number=OrderNumber("ORD-001"),
        quantity=Quantity(5),
        total_price=TotalPrice(PyDecimal("499.95")),
        order_date=OrderDate(date(2024, 1, 15)),
    )
    manager.insert(order)

    # Fetch and verify
    results = manager.get(order_number="ORD-001")
    assert len(results) == 1
    assert results[0].quantity.value == 5
    assert results[0].total_price.value == PyDecimal("499.95")
    assert results[0].order_date.value == date(2024, 1, 15)


@pytest.mark.integration
@pytest.mark.order(88)
def test_session_entity_string_datetime_datetimetz_duration(clean_db):
    """Test Session entity with String, DateTime, DateTimeTZ, and Duration types."""

    class SessionId(String):
        pass

    class StartedAt(DateTime):
        pass

    class LastActive(DateTimeTZ):
        pass

    class SessionDuration(Duration):
        pass

    class Session(Entity):
        flags = TypeFlags(name="session_combo")
        session_id: SessionId = Flag(Key)
        started_at: StartedAt
        last_active: LastActive
        duration: SessionDuration

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Session)
    schema_manager.sync_schema(force=True)

    manager = Session.manager(clean_db)

    # Insert session
    session = Session(
        session_id=SessionId("sess-001"),
        started_at=StartedAt(datetime(2024, 1, 1, 10, 0, 0)),
        last_active=LastActive(datetime(2024, 1, 1, 12, 30, 0, tzinfo=UTC)),
        duration=SessionDuration(timedelta(hours=2, minutes=30)),
    )
    manager.insert(session)

    # Fetch and verify
    results = manager.get(session_id="sess-001")
    assert len(results) == 1
    assert results[0].started_at.value == datetime(2024, 1, 1, 10, 0, 0)
    assert results[0].duration.value == timedelta(hours=2, minutes=30)


@pytest.mark.integration
@pytest.mark.order(89)
def test_score_entity_string_integer_double_boolean(clean_db):
    """Test Score entity with String, Integer, Double, and Boolean types."""

    class PlayerName(String):
        pass

    class Level(Integer):
        pass

    class Points(Double):
        pass

    class IsWinner(Boolean):
        pass

    class Score(Entity):
        flags = TypeFlags(name="score_combo")
        player: PlayerName = Flag(Key)
        level: Level
        points: Points
        is_winner: IsWinner

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Score)
    schema_manager.sync_schema(force=True)

    manager = Score.manager(clean_db)

    # Insert score
    score = Score(
        player=PlayerName("Alice"),
        level=Level(10),
        points=Points(9850.5),
        is_winner=IsWinner(True),
    )
    manager.insert(score)

    # Fetch and verify
    results = manager.get(player="Alice")
    assert len(results) == 1
    assert results[0].level.value == 10
    assert abs(results[0].points.value - 9850.5) < 0.01
    assert results[0].is_winner.value is True


@pytest.mark.integration
@pytest.mark.order(90)
def test_audit_entity_all_nine_types(clean_db):
    """Test Audit entity with all 9 attribute types."""

    class AuditId(String):
        pass

    class UserId(Integer):
        pass

    class IsSuccessful(Boolean):
        pass

    class RiskScore(Double):
        pass

    class ActionDate(Date):
        pass

    class ActionTime(DateTime):
        pass

    class SyncedAt(DateTimeTZ):
        pass

    class Amount(Decimal):
        pass

    class ProcessingTime(Duration):
        pass

    class Audit(Entity):
        flags = TypeFlags(name="audit_all")
        audit_id: AuditId = Flag(Key)
        user_id: UserId
        is_successful: IsSuccessful
        risk_score: RiskScore
        action_date: ActionDate
        action_time: ActionTime
        synced_at: SyncedAt
        amount: Amount
        processing_time: ProcessingTime

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Audit)
    schema_manager.sync_schema(force=True)

    manager = Audit.manager(clean_db)

    # Insert audit record with all 9 types
    audit = Audit(
        audit_id=AuditId("AUD-001"),
        user_id=UserId(12345),
        is_successful=IsSuccessful(True),
        risk_score=RiskScore(0.25),
        action_date=ActionDate(date(2024, 1, 15)),
        action_time=ActionTime(datetime(2024, 1, 15, 10, 30, 0)),
        synced_at=SyncedAt(datetime(2024, 1, 15, 10, 31, 0, tzinfo=UTC)),
        amount=Amount(PyDecimal("1500.00")),
        processing_time=ProcessingTime(timedelta(milliseconds=250)),
    )
    manager.insert(audit)

    # Fetch and verify all types
    results = manager.get(audit_id="AUD-001")
    assert len(results) == 1
    r = results[0]
    assert r.user_id.value == 12345
    assert r.is_successful.value is True
    assert abs(r.risk_score.value - 0.25) < 0.01
    assert r.action_date.value == date(2024, 1, 15)
    assert r.action_time.value == datetime(2024, 1, 15, 10, 30, 0)
    assert r.amount.value == PyDecimal("1500.00")
    assert r.processing_time.value == timedelta(milliseconds=250)
