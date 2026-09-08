"""Generated constructors enforce the Split-YAML scalar constraints."""

from app_models import Age, DisplayName, Person, PersonId
from type_bridge_core import MatchRequestError


def main() -> None:
    Person(person_id=PersonId("ada"), display_name=DisplayName("Ada"), age=Age(36))
    try:
        Age(151)
    except MatchRequestError as error:
        print("generated range validation:", error)


if __name__ == "__main__":
    main()
