"""Aggregate expressions for database-side calculations."""

from typing import TYPE_CHECKING, Literal

from type_bridge.expressions.base import Expression

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute


class AggregateExpr[T: "Attribute"](Expression):
    """
    Type-safe aggregate expression for database-side calculations.

    Represents aggregations like sum(age), avg(score), count(*), etc.
    """

    def __init__(
        self,
        attr_type: type[T] | None = None,
        function: Literal["sum", "mean", "max", "min", "count", "median", "std"] = "count",
        field_name: str | None = None,
    ):
        """
        Create an aggregate expression.

        Args:
            attr_type: Attribute type to aggregate (None for count)
            function: Aggregation function (use 'mean' for TypeDB 3.x, not 'avg')
            field_name: Python field name (used for result keys, e.g., 'salary' → 'avg_salary')

        Note:
            The user-facing avg() method automatically converts to 'mean'.
            TypeDB 3.x uses 'mean' instead of 'avg'.
        """
        self.attr_type = attr_type
        self.function = function
        self.field_name = field_name

        # Validate attr_type requirement
        if function != "count" and attr_type is None:
            raise ValueError(f"{function.upper()} requires an attribute type")

    def to_typeql(self, var: str) -> str:
        """
        Generate TypeQL pattern for this aggregation.

        This is used in fetch clauses, not match clauses.

        Example output: "mean($p_age)"

        Args:
            var: Entity variable name (used to reference attributes)

        Returns:
            TypeQL aggregation expression
        """
        if self.function == "count":
            # Count all matches
            return f"count({var})"

        # Other aggregations need attribute reference
        assert self.attr_type is not None
        from type_bridge.expressions.utils import generate_attr_var

        attr_var = generate_attr_var(var, self.attr_type)
        return f"{self.function}({attr_var})"

    def get_fetch_key(self) -> str:
        """
        Get the key to use in fetch results.

        Note:
            AUTOMATIC CONVERSION: TypeQL 'mean' → result key 'avg'
            For user-facing consistency, result keys use 'avg' even though
            TypeDB internally uses 'mean'. This matches the method name.

        Returns:
            String key for accessing aggregate result

        Example:
            Person.salary.avg() generates TypeQL 'mean($personsalary)'
            but result key is 'avg_salary' (using field name, not attribute type name)
        """
        if self.function == "count":
            return "count"

        assert self.attr_type is not None
        # AUTOMATIC CONVERSION: Map 'mean' back to 'avg' for user-facing API
        func_name = "avg" if self.function == "mean" else self.function

        # Use field_name if available (from FieldRef), otherwise fall back to attribute type name
        if self.field_name:
            return f"{func_name}_{self.field_name}"
        else:
            attr_type_name = self.attr_type.get_attribute_name()
            return f"{func_name}_{attr_type_name.lower()}"
