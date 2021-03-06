use crate::analyser::scope::Scope;
use crate::analyser::sym_resolver::{TypeInfo, VarInfo, VarKind};
use crate::ast::types::TypeLitNum;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn scope_test() {
    let mut scope = Scope::new(0);
    let var_info = VarInfo::new(3, VarKind::Local, Rc::new(RefCell::new(TypeInfo::LitNum(TypeLitNum::U64))));

    scope.cur_stmt_id = 1;
    scope.add_variable("a", VarKind::Local, Rc::new(RefCell::new(TypeInfo::Bool)));
    scope.cur_stmt_id = 3;
    scope.add_variable("a", VarKind::Local, Rc::new(RefCell::new(TypeInfo::LitNum(TypeLitNum::U64))));
    scope.cur_stmt_id = 8;
    scope.add_variable("a", VarKind::LocalMut, Rc::new(RefCell::new(TypeInfo::Bool)));
    scope.cur_stmt_id = 4;
    assert_eq!(&var_info, scope.find_variable("a").unwrap().0);
}
