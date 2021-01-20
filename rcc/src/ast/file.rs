use crate::parser::item::Item;

#[derive(Debug, PartialEq)]
pub struct File<'a> {
    pub items: Vec<Item<'a>>,
}
