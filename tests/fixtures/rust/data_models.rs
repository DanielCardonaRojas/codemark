pub struct User {
    pub id: u64,
    pub username: String,
}

pub enum Role {
    Admin,
    User(u64),
    Guest { temporary: bool },
}

pub trait Auth {
    fn login(&self) -> bool;
}

impl Auth for User {
    fn login(&self) -> bool {
        true
    }
}
