"""Performance tests for attribute validation.

These tests verify that attribute validation doesn't become a bottleneck
for high-throughput data ingestion scenarios.
"""

import time

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


class Name(String):
    pass


class Age(Integer):
    pass


class Score(Integer):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None
    score: Score | None = None


class TestAttributeValidationPerformance:
    """Performance tests for attribute validation overhead."""

    def test_attribute_instantiation_performance(self):
        """Attribute instantiation should be fast enough for bulk operations.

        Target: At least 10,000 attribute instances per second.
        """
        iterations = 10000

        start = time.perf_counter()
        for i in range(iterations):
            Name(f"Person{i}")
            Age(i % 100)
            Score(i * 10)
        elapsed = time.perf_counter() - start

        # Calculate rate
        rate = (iterations * 3) / elapsed  # 3 attributes per iteration
        print(f"\nAttribute instantiation: {rate:.0f} attrs/sec ({elapsed:.3f}s for {iterations * 3} attrs)")

        # Should be able to create at least 10k attributes per second
        assert rate > 10000, f"Attribute instantiation too slow: {rate:.0f}/sec"

    def test_entity_instantiation_performance(self):
        """Entity instantiation with attributes should be reasonably fast.

        Target: At least 1,000 entities per second.
        """
        iterations = 1000

        start = time.perf_counter()
        for i in range(iterations):
            Person(
                name=Name(f"Person{i}"),
                age=Age(i % 100),
                score=Score(i * 10),
            )
        elapsed = time.perf_counter() - start

        rate = iterations / elapsed
        print(f"\nEntity instantiation: {rate:.0f} entities/sec ({elapsed:.3f}s for {iterations} entities)")

        # Should be able to create at least 1k entities per second
        assert rate > 1000, f"Entity instantiation too slow: {rate:.0f}/sec"

    def test_pydantic_validate_performance(self):
        """_pydantic_validate should not add significant overhead.

        Compares direct instantiation vs _pydantic_validate.
        """
        iterations = 5000

        # Direct instantiation
        start = time.perf_counter()
        for i in range(iterations):
            Name(f"Test{i}")
        direct_time = time.perf_counter() - start

        # Via _pydantic_validate
        start = time.perf_counter()
        for i in range(iterations):
            Name._pydantic_validate(f"Test{i}")
        validate_time = time.perf_counter() - start

        overhead = (validate_time / direct_time - 1) * 100
        print(f"\n_pydantic_validate overhead: {overhead:.1f}%")
        print(f"  Direct: {iterations / direct_time:.0f}/sec")
        print(f"  Validate: {iterations / validate_time:.0f}/sec")

        # Overhead should be reasonable (less than 100% slower)
        assert overhead < 100, f"_pydantic_validate overhead too high: {overhead:.1f}%"

    def test_bulk_entity_creation(self):
        """Bulk entity creation simulating data import scenario."""
        # Simulate importing 5000 records
        records = [
            {"name": f"Person{i}", "age": i % 100, "score": i * 10}
            for i in range(5000)
        ]

        start = time.perf_counter()
        entities = []
        for record in records:
            entity = Person(
                name=Name(record["name"]),
                age=Age(record["age"]),
                score=Score(record["score"]),
            )
            entities.append(entity)
        elapsed = time.perf_counter() - start

        rate = len(records) / elapsed
        print(f"\nBulk import: {rate:.0f} records/sec ({elapsed:.3f}s for {len(records)} records)")

        # Should handle at least 1000 records per second
        assert rate > 1000, f"Bulk import too slow: {rate:.0f}/sec"
        assert len(entities) == 5000
