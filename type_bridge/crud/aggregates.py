"""Aggregate result parsing for TypeDB 3.8.0+."""

from typing import Any


def parse_aggregate_results(
    results: list[dict[str, Any]],
) -> dict[str, Any]:
    """Parse aggregation results from TypeDB reduce query.

    Args:
        results: Raw results from TypeDB reduce query

    Returns:
        Dictionary mapping aggregate variable names to their values
    """
    if not results:
        return {}

    result = results[0]
    output: dict[str, Any] = {}

    for var_name, concept_data in result.items():
        if isinstance(concept_data, dict):
            output[var_name] = concept_data.get("value")
        else:
            output[var_name] = concept_data

    return output


def parse_grouped_aggregate_results(
    results: list[dict[str, Any]],
    group_vars: list[str],
) -> dict[Any, dict[str, Any]]:
    """Parse grouped aggregation results from TypeDB reduce groupby query.

    Args:
        results: Raw results from TypeDB reduce groupby query
        group_vars: List of group variable names (e.g., ["$group0", "$group1"])

    Returns:
        Dictionary mapping group keys to aggregation results
    """
    output: dict[Any, dict[str, Any]] = {}
    group_var_names = {var.lstrip("$") for var in group_vars}

    for result in results:
        group_keys: list[Any] = []
        group_aggs: dict[str, Any] = {}

        for var_name, concept_data in result.items():
            if isinstance(concept_data, dict):
                value = concept_data.get("value")
            else:
                value = concept_data

            if var_name in group_var_names:
                group_keys.append(value)
            else:
                group_aggs[var_name] = value

        group_key: Any = group_keys[0] if len(group_keys) == 1 else tuple(group_keys)
        output[group_key] = group_aggs

    return output
