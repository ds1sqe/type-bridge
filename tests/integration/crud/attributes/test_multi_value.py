"""Integration tests for multi-value attributes across all types."""

from datetime import UTC, date, datetime, timedelta
from decimal import Decimal as PyDecimal

import pytest

from type_bridge import (
    Boolean,
    Card,
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
@pytest.mark.order(59)
def test_multi_value_boolean(clean_db):
    """Test multi-value Boolean attributes."""

    class Name(String):
        pass

    class Feature(Boolean):
        pass

    class Config(Entity):
        flags = TypeFlags(name="config_bool")
        name: Name = Flag(Key)
        features: list[Feature] = Flag(Card(min=1))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Config)
    schema_manager.sync_schema(force=True)

    manager = Config.manager(clean_db)

    # Insert with multiple Boolean values
    config = Config(name=Name("app1"), features=[Feature(True), Feature(False)])
    manager.insert(config)

    results = manager.get(name="app1")
    assert len(results) == 1
    assert len(results[0].features) == 2


@pytest.mark.integration
@pytest.mark.order(60)
def test_multi_value_integer(clean_db):
    """Test multi-value Integer attributes."""

    class Name(String):
        pass

    class Score(Integer):
        pass

    class Student(Entity):
        flags = TypeFlags(name="student_scores")
        name: Name = Flag(Key)
        scores: list[Score] = Flag(Card(min=1, max=5))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Student)
    schema_manager.sync_schema(force=True)

    manager = Student.manager(clean_db)

    # Insert with multiple Integer values
    student = Student(name=Name("Alice"), scores=[Score(85), Score(90), Score(78)])
    manager.insert(student)

    results = manager.get(name="Alice")
    assert len(results) == 1
    assert len(results[0].scores) == 3
    score_values = {s.value for s in results[0].scores}
    assert score_values == {85, 90, 78}


@pytest.mark.integration
@pytest.mark.order(61)
def test_multi_value_string(clean_db):
    """Test multi-value String attributes."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Article(Entity):
        flags = TypeFlags(name="article_tags")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=1))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Article)
    schema_manager.sync_schema(force=True)

    manager = Article.manager(clean_db)

    # Insert with multiple String values
    article = Article(
        name=Name("TypeDB Tutorial"), tags=[Tag("database"), Tag("python"), Tag("orm")]
    )
    manager.insert(article)

    results = manager.get(name="TypeDB Tutorial")
    assert len(results) == 1
    assert len(results[0].tags) == 3
    tag_values = {t.value for t in results[0].tags}
    assert tag_values == {"database", "python", "orm"}


@pytest.mark.integration
@pytest.mark.order(62)
def test_multi_value_double(clean_db):
    """Test multi-value Double attributes."""

    class Name(String):
        pass

    class Measurement(Double):
        pass

    class Experiment(Entity):
        flags = TypeFlags(name="experiment_measures")
        name: Name = Flag(Key)
        measurements: list[Measurement] = Flag(Card(min=2))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Experiment)
    schema_manager.sync_schema(force=True)

    manager = Experiment.manager(clean_db)

    # Insert with multiple Double values
    experiment = Experiment(
        name=Name("Test1"),
        measurements=[Measurement(1.5), Measurement(2.7), Measurement(3.9)],
    )
    manager.insert(experiment)

    results = manager.get(name="Test1")
    assert len(results) == 1
    assert len(results[0].measurements) == 3


@pytest.mark.integration
@pytest.mark.order(63)
def test_multi_value_date(clean_db):
    """Test multi-value Date attributes."""

    class Name(String):
        pass

    class EventDate(Date):
        pass

    class Project(Entity):
        flags = TypeFlags(name="project_dates")
        name: Name = Flag(Key)
        milestones: list[EventDate] = Flag(Card(min=1))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Project)
    schema_manager.sync_schema(force=True)

    manager = Project.manager(clean_db)

    # Insert with multiple Date values
    project = Project(
        name=Name("Launch"),
        milestones=[
            EventDate(date(2024, 1, 15)),
            EventDate(date(2024, 3, 1)),
            EventDate(date(2024, 6, 1)),
        ],
    )
    manager.insert(project)

    results = manager.get(name="Launch")
    assert len(results) == 1
    assert len(results[0].milestones) == 3


@pytest.mark.integration
@pytest.mark.order(64)
def test_multi_value_datetime(clean_db):
    """Test multi-value DateTime attributes."""

    class Name(String):
        pass

    class Timestamp(DateTime):
        pass

    class Log(Entity):
        flags = TypeFlags(name="log_timestamps")
        name: Name = Flag(Key)
        timestamps: list[Timestamp] = Flag(Card(min=1))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Log)
    schema_manager.sync_schema(force=True)

    manager = Log.manager(clean_db)

    # Insert with multiple DateTime values
    log = Log(
        name=Name("access_log"),
        timestamps=[
            Timestamp(datetime(2024, 1, 1, 10, 0, 0)),
            Timestamp(datetime(2024, 1, 1, 11, 0, 0)),
            Timestamp(datetime(2024, 1, 1, 12, 0, 0)),
        ],
    )
    manager.insert(log)

    results = manager.get(name="access_log")
    assert len(results) == 1
    assert len(results[0].timestamps) == 3


@pytest.mark.integration
@pytest.mark.order(65)
def test_multi_value_datetimetz(clean_db):
    """Test multi-value DateTimeTZ attributes."""

    class Name(String):
        pass

    class SyncTime(DateTimeTZ):
        pass

    class Cluster(Entity):
        flags = TypeFlags(name="cluster_syncs")
        name: Name = Flag(Key)
        sync_times: list[SyncTime] = Flag(Card(min=1))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Cluster)
    schema_manager.sync_schema(force=True)

    manager = Cluster.manager(clean_db)

    # Insert with multiple DateTimeTZ values
    cluster = Cluster(
        name=Name("prod"),
        sync_times=[
            SyncTime(datetime(2024, 1, 1, 10, 0, 0, tzinfo=UTC)),
            SyncTime(datetime(2024, 1, 1, 14, 0, 0, tzinfo=UTC)),
        ],
    )
    manager.insert(cluster)

    results = manager.get(name="prod")
    assert len(results) == 1
    assert len(results[0].sync_times) == 2


@pytest.mark.integration
@pytest.mark.order(66)
def test_multi_value_decimal(clean_db):
    """Test multi-value Decimal attributes."""

    class Name(String):
        pass

    class Price(Decimal):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product_prices")
        name: Name = Flag(Key)
        price_history: list[Price] = Flag(Card(min=1))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Product)
    schema_manager.sync_schema(force=True)

    manager = Product.manager(clean_db)

    # Insert with multiple Decimal values
    product = Product(
        name=Name("Laptop"),
        price_history=[
            Price(PyDecimal("999.99")),
            Price(PyDecimal("899.99")),
            Price(PyDecimal("849.99")),
        ],
    )
    manager.insert(product)

    results = manager.get(name="Laptop")
    assert len(results) == 1
    assert len(results[0].price_history) == 3


@pytest.mark.integration
@pytest.mark.order(67)
def test_multi_value_duration(clean_db):
    """Test multi-value Duration attributes."""

    class Name(String):
        pass

    class Interval(Duration):
        pass

    class Schedule(Entity):
        flags = TypeFlags(name="schedule_intervals")
        name: Name = Flag(Key)
        intervals: list[Interval] = Flag(Card(min=1))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Schedule)
    schema_manager.sync_schema(force=True)

    manager = Schedule.manager(clean_db)

    # Insert with multiple Duration values
    schedule = Schedule(
        name=Name("meetings"),
        intervals=[
            Interval(timedelta(minutes=30)),
            Interval(timedelta(hours=1)),
            Interval(timedelta(hours=2)),
        ],
    )
    manager.insert(schedule)

    results = manager.get(name="meetings")
    assert len(results) == 1
    assert len(results[0].intervals) == 3
