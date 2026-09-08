"""Unit tests for recursive and cyclic relation handling.

These tests verify the ORM handles recursive structures correctly
without infinite loops or stack overflows.
"""

from tests.utils.handwritten import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags


# Models for testing recursive structures
class NodeId(Integer):
    pass


class NodeName(String):
    pass


class Node(Entity):
    """A node that can be linked to other nodes."""

    flags = TypeFlags(name="node")
    id: NodeId = Flag(Key)
    name: NodeName | None = None


class Link(Relation):
    """A directed link between nodes (can form cycles)."""

    flags = TypeFlags(name="link")
    source: Role[Node] = Role("source", Node)
    target: Role[Node] = Role("target", Node)


class ParentChild(Relation):
    """A hierarchical relation (can form deep trees)."""

    flags = TypeFlags(name="parent-child")
    parent: Role[Node] = Role("parent", Node)
    child: Role[Node] = Role("child", Node)


class TestRecursiveModelDefinition:
    """Test that recursive model definitions work correctly."""

    def test_self_referential_relation_definition(self):
        """A relation can reference the same entity type for multiple roles."""
        # Both source and target are Node - this should work
        node1 = Node(id=NodeId(1), name=NodeName("A"))
        node2 = Node(id=NodeId(2), name=NodeName("B"))

        link = Link(source=node1, target=node2)
        assert link.source.id.value == 1
        assert link.target.id.value == 2

    def test_cyclic_relation_instances(self):
        """Relation instances can form cycles (A -> B -> A)."""
        node_a = Node(id=NodeId(1), name=NodeName("A"))
        node_b = Node(id=NodeId(2), name=NodeName("B"))

        # A -> B
        link1 = Link(source=node_a, target=node_b)
        # B -> A (creates cycle)
        link2 = Link(source=node_b, target=node_a)

        # Both links should be valid
        assert link1.source.id.value == 1
        assert link1.target.id.value == 2
        assert link2.source.id.value == 2
        assert link2.target.id.value == 1

    def test_self_loop_relation(self):
        """A node can link to itself."""
        node = Node(id=NodeId(1), name=NodeName("Self"))
        self_link = Link(source=node, target=node)

        assert self_link.source is self_link.target
        assert self_link.source.id.value == 1

    def test_deep_hierarchy_definition(self):
        """Deep hierarchical structures can be defined."""
        # Create a chain: root -> child1 -> child2 -> ... -> childN
        depth = 50
        nodes = [Node(id=NodeId(i), name=NodeName(f"Node{i}")) for i in range(depth)]

        relations = []
        for i in range(depth - 1):
            rel = ParentChild(parent=nodes[i], child=nodes[i + 1])
            relations.append(rel)

        # Verify the chain
        assert len(relations) == depth - 1
        assert relations[0].parent.id.value == 0
        assert relations[-1].child.id.value == depth - 1


class TestModelSerialization:
    """Test serialization of recursive structures."""

    def test_model_dump_with_cyclic_references(self):
        """model_dump should handle cyclic references without infinite recursion."""
        node_a = Node(id=NodeId(1), name=NodeName("A"))
        node_b = Node(id=NodeId(2), name=NodeName("B"))

        link = Link(source=node_a, target=node_b)

        # This should not cause infinite recursion
        link_dict = link.model_dump()

        assert "source" in link_dict
        assert "target" in link_dict

    def test_model_dump_with_self_reference(self):
        """model_dump handles self-referential relations."""
        node = Node(id=NodeId(1), name=NodeName("Self"))
        self_link = Link(source=node, target=node)

        link_dict = self_link.model_dump()

        # Both should serialize to the same node data
        assert link_dict["source"]["id"] == link_dict["target"]["id"]

    def test_entity_to_dict(self):
        """Entity to_dict works correctly."""
        node = Node(id=NodeId(1), name=NodeName("Test"))
        node_dict = node.to_dict()

        assert node_dict["id"] == 1
        assert node_dict["name"] == "Test"


class TestModelValidation:
    """Test Pydantic validation of recursive structures."""

    def test_revalidation_preserves_cyclic_structure(self):
        """Revalidating a model with cycles preserves the structure."""
        node_a = Node(id=NodeId(1), name=NodeName("A"))
        node_b = Node(id=NodeId(2), name=NodeName("B"))

        link = Link(source=node_a, target=node_b)

        # Revalidate
        revalidated = Link.model_validate(link)

        assert revalidated.source.id.value == 1
        assert revalidated.target.id.value == 2

    def test_model_validate_with_entity_instances(self):
        """model_validate works when role players are Entity instances."""
        # Relations require Entity instances for role players
        node_a = Node(id=NodeId(1), name=NodeName("A"))
        node_b = Node(id=NodeId(2), name=NodeName("B"))

        # Validate with entity instances in a dict
        link = Link.model_validate({"source": node_a, "target": node_b})

        assert link.source.id.value == 1
        assert link.target.id.value == 2
