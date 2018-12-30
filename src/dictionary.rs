use crate::shape::Shape;
use std::collections::HashMap;
use std::ops::Index;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharacterId(pub u16);

#[derive(Clone, Debug)]
pub enum Character<'a> {
    Shape(Shape<'a>),
}

#[derive(Clone, Default, Debug)]
pub struct Dictionary<'a> {
    characters: HashMap<CharacterId, Character<'a>>,
}

impl<'a> Dictionary<'a> {
    pub fn define(&mut self, id: CharacterId, character: Character<'a>) {
        assert!(
            self.characters.insert(id, character).is_none(),
            "Dictionary::define: ID {} is already taken"
        );
    }
}

impl<'a> Index<CharacterId> for Dictionary<'a> {
    type Output = Character<'a>;
    fn index(&self, id: CharacterId) -> &Self::Output {
        &self.characters[&id]
    }
}
