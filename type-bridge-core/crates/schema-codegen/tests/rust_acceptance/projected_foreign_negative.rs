fn construction(reference: foreign::PersonRef) {
    let _ = generated::PlainActivityCreate::new(reference);
}

fn hydration(person: foreign::Person) -> generated::Person {
    person
}

fn main() {}
