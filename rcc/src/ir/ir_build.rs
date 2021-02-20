use crate::analyser::scope::ScopeStack;
use crate::analyser::sym_resolver::{TypeInfo, VarKind};
use crate::ast::expr::{
    ArrayExpr, ArrayIndexExpr, AssignExpr, AssignOp, BinOpExpr, BinOperator, BlockExpr, BreakExpr,
    CallExpr, Expr, ExprKind, ExprVisit, FieldAccessExpr, GroupedExpr, IfExpr, LhsExpr, LitNumExpr,
    LoopExpr, PathExpr, RangeExpr, ReturnExpr, StructExpr, TupleExpr, TupleIndexExpr, UnAryExpr,
    WhileExpr,
};
use crate::ast::file::File;
use crate::ast::item::{Item, ItemFn, ItemStruct};
use crate::ast::pattern::{IdentPattern, Pattern};
use crate::ast::stmt::{LetStmt, Stmt};
use crate::ast::types::TypeLitNum;
use crate::ast::AST;
use crate::ir::Jump::*;
use crate::ir::{Func, IRInst, IRType, Jump, Operand, Place, IR};
use crate::rcc::{OptimizeLevel, RccError};
use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

pub struct IRBuilder {
    ir_output: IR,
    fn_ret_temp_var: Vec<Place>,

    scope_stack: ScopeStack,

    // (place = loop expr, break link)
    loop_var_stack: Vec<(Option<Place>, usize)>,

    optimize_level: OptimizeLevel,
}

impl IRBuilder {
    pub fn new(optimize_level: OptimizeLevel) -> IRBuilder {
        IRBuilder {
            ir_output: IR::new(),
            fn_ret_temp_var: vec![],
            scope_stack: ScopeStack::new(),
            loop_var_stack: vec![],
            optimize_level,
        }
    }

    pub(crate) fn generate_ir(&mut self, ast: &mut AST) -> Result<IR, RccError> {
        self.visit_file(&mut ast.file)?;
        let mut output = IR::new();
        std::mem::swap(&mut self.ir_output, &mut output);
        Ok(output)
    }

    fn gen_temp_variable(&mut self, type_info: Rc<RefCell<TypeInfo>>) -> Place {
        let label = self
            .scope_stack
            .cur_scope_mut()
            .gen_temp_variable(type_info);
        Place::local(label)
    }

    fn gen_variable(&mut self, ident: &str, var_kind: VarKind) -> Place {
        let res = self.scope_stack.cur_scope().find_variable(ident).unwrap();
        Place::variable(ident, res.1, var_kind)
    }

    fn visit_file(&mut self, file: &mut File) -> Result<(), RccError> {
        self.scope_stack.enter_file(file);
        for item in file.items.iter_mut() {
            self.visit_item(item)?;
        }
        Ok(())
    }

    fn visit_item(&mut self, item: &mut Item) -> Result<(), RccError> {
        match item {
            Item::Fn(item_fn) => self.visit_item_fn(item_fn),
            Item::Struct(item_struct) => self.visit_item_struct(item_struct),
            _ => unimplemented!(),
        }
    }

    fn visit_item_fn(&mut self, item_fn: &mut ItemFn) -> Result<(), RccError> {
        self.ir_output.add_func(item_fn.name.clone());

        let info = self.scope_stack.cur_scope().find_fn(&item_fn.name);
        assert_eq!(info, TypeInfo::from_item_fn(item_fn));

        let dest = self.gen_temp_variable(item_fn.fn_block.type_info());
        self.fn_ret_temp_var.push(dest.clone());
        let operand = self.visit_block_expr(&mut item_fn.fn_block, Some(dest))?;

        if item_fn.fn_block.last_expr.is_none() && item_fn.fn_block.stmts.is_empty() {
            self.ir_output.add_instructions(IRInst::Ret(Operand::Unit));
        } else if !item_fn.fn_block.last_stmt_is_return() {
            self.ir_output.add_instructions(IRInst::Ret(operand));
        }

        self.fn_ret_temp_var.pop();
        Ok(())
    }

    fn visit_item_struct(&mut self, item_struct: &mut ItemStruct) -> Result<(), RccError> {
        unimplemented!()
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) -> Result<(), RccError> {
        match stmt {
            Stmt::Semi => Ok(()),
            Stmt::Item(item) => self.visit_item(item),
            Stmt::Let(let_stmt) => self.visit_let_stmt(let_stmt),
            Stmt::ExprStmt(expr) => {
                let operand = self.visit_expr(expr, None)?;
                debug_assert!(operand.is_unit_or_never(), "{:?}", expr);
                Ok(())
            }
        }
    }

    fn visit_let_stmt(&mut self, let_stmt: &mut LetStmt) -> Result<(), RccError> {
        let is_mut = let_stmt.is_mut();
        if let Some(rhs) = &mut let_stmt.rhs {
            match &let_stmt.pattern {
                Pattern::Identifier(ident_pattern) => {
                    let ident = ident_pattern.ident();
                    let dest = self.gen_variable(
                        ident,
                        if is_mut {
                            VarKind::LocalMut
                        } else {
                            VarKind::Local
                        },
                    );
                    let rhs = self.visit_expr(rhs, Some(dest))?;
                }
            }
        }
        Ok(())
    }

    fn visit_expr(&mut self, expr: &mut Expr, dest: Option<Place>) -> Result<Operand, RccError> {
        let result = match expr {
            Expr::Path(path_expr) => self.visit_path_expr(path_expr, dest),
            Expr::LitNum(lit_num_expr) => self.visit_lit_num_expr(lit_num_expr, dest),
            Expr::LitBool(lit_bool) => self.visit_lit_bool(lit_bool, dest),
            Expr::LitChar(lit_char) => self.visit_lit_char(lit_char, dest),
            Expr::LitStr(s) => self.visit_lit_str(s, dest),
            Expr::Unary(unary_expr) => self.visit_unary_expr(unary_expr),
            Expr::Block(block_expr) => self.visit_block_expr(block_expr, dest),
            Expr::Assign(assign_expr) => self.visit_assign_expr(assign_expr),
            // Expr::Range(range_expr) => self.visit_range_expr(range_expr),
            Expr::BinOp(bin_op_expr) => self.visit_bin_op_expr(bin_op_expr, dest),
            Expr::Grouped(grouped_expr) => self.visit_grouped_expr(grouped_expr, dest),
            // Expr::Array(array_expr) => self.visit_array_expr(array_expr),
            // Expr::ArrayIndex(array_index_expr) => self.visit_array_index_expr(array_index_expr),
            // Expr::Tuple(tuple_expr) => self.visit_tuple_expr(tuple_expr),
            // Expr::TupleIndex(tuple_index_expr) => self.visit_tuple_index_expr(tuple_index_expr),
            // Expr::Struct(struct_expr) => self.visit_struct_expr(struct_expr),
            Expr::Call(call_expr) => self.visit_call_expr(call_expr, dest),
            // Expr::FieldAccess(field_access_expr) => self.visit_field_access_expr(field_access_expr),
            Expr::While(while_expr) => self.visit_while_expr(while_expr),
            Expr::Loop(loop_expr) => self.visit_loop_expr(loop_expr, dest),
            Expr::If(if_expr) => self.visit_if_expr(if_expr, dest),
            Expr::Return(return_expr) => self.visit_return_expr(return_expr, dest),
            Expr::Break(break_expr) => self.visit_break_expr(break_expr, dest),
            _ => unimplemented!(),
        };
        debug_assert_ne!(
            ExprKind::Unknown,
            expr.kind(),
            "unknown expr kind: {:?}",
            expr
        );
        result
    }

    fn visit_lhs_expr(&mut self, lhs_expr: &mut LhsExpr) -> Result<Operand, RccError> {
        let r = match lhs_expr {
            LhsExpr::Path(expr) => self.visit_path_expr(expr, None)?,
            _ => todo!("visit lhs expr"),
        };
        Ok(r)
    }

    fn visit_grouped_expr(
        &mut self,
        grouped_expr: &mut GroupedExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        self.visit_expr(grouped_expr, dest)
    }

    fn visit_pattern(&mut self, pattern: &mut Pattern) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_ident_pattern(
        &mut self,
        ident_pattern: &mut IdentPattern,
    ) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_path_expr(
        &mut self,
        path_expr: &mut PathExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        // TODO path segmentation
        let ident = path_expr.segments.last().unwrap();

        let cur_scope = self.scope_stack.cur_scope();
        if let Some((var, scope_id)) = cur_scope.find_variable(ident) {
            let operand = Operand::Place(Place::variable(ident, scope_id, var.kind()));
            if let Some(d) = dest {
                if !d.is_temp() {
                    self.ir_output
                        .add_instructions(IRInst::load_data(d, operand.clone()));
                }
            }
            Ok(operand)
        } else if !cur_scope.find_fn(ident).is_unknown() {
            Ok(Operand::FnLabel(ident.clone()))
        } else {
            Err("error in visit path expr: ident not found".into())
        }
    }

    fn lit(&mut self, operand: Operand, d: Place) -> Result<Operand, RccError> {
        if !d.is_temp() {
            self.ir_output
                .add_instructions(IRInst::load_data(d, operand.clone()));
        }
        Ok(operand)
    }

    fn visit_lit_num_expr(
        &mut self,
        lit_num_expr: &mut LitNumExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        match dest {
            Some(d) => {
                let t = lit_num_expr.get_lit_type();
                let operand = match t {
                    TypeLitNum::I8 => Operand::I8(lit_num_expr.value.parse()?),
                    TypeLitNum::I16 => Operand::I16(lit_num_expr.value.parse()?),
                    TypeLitNum::I | TypeLitNum::I32 => Operand::I32(lit_num_expr.value.parse()?),
                    TypeLitNum::I64 => Operand::I64(lit_num_expr.value.parse()?),
                    TypeLitNum::I128 => Operand::I128(lit_num_expr.value.parse()?),
                    TypeLitNum::Isize => Operand::Isize(lit_num_expr.value.parse()?),
                    TypeLitNum::U8 => Operand::U8(lit_num_expr.value.parse()?),
                    TypeLitNum::U16 => Operand::U16(lit_num_expr.value.parse()?),
                    TypeLitNum::U32 => Operand::U32(lit_num_expr.value.parse()?),
                    TypeLitNum::U64 => Operand::U64(lit_num_expr.value.parse()?),
                    TypeLitNum::U128 => Operand::U128(lit_num_expr.value.parse()?),
                    TypeLitNum::Usize => Operand::Usize(lit_num_expr.value.parse()?),
                    TypeLitNum::F32 => Operand::F32(lit_num_expr.value.parse()?),
                    TypeLitNum::F | TypeLitNum::F64 => Operand::F64(lit_num_expr.value.parse()?),
                };
                self.lit(operand, d)
            }
            None => Ok(Operand::Unit),
        }
    }

    fn visit_lit_bool(
        &mut self,
        lit_bool: &mut bool,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        match dest {
            Some(d) => self.lit(Operand::Bool(*lit_bool), d),
            None => Ok(Operand::Unit),
        }
    }

    fn visit_lit_char(
        &mut self,
        lit_char: &mut char,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        match dest {
            Some(d) => self.lit(Operand::Char(*lit_char), d),
            None => Ok(Operand::Unit),
        }
    }

    fn visit_lit_str(&mut self, s: &str, _dest: Option<Place>) -> Result<Operand, RccError> {
        Ok(self.ir_output.add_ro_local_str(s.to_string()))
    }

    fn visit_unary_expr(&mut self, unary_expr: &mut UnAryExpr) -> Result<Operand, RccError> {
        // let d = self.gen_temp_variable(unary_expr.expr.type_info());
        // let operand = self.visit_expr(&mut unary_expr.expr, d)?;
        todo!()
    }

    fn visit_block_expr(
        &mut self,
        block_expr: &mut BlockExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        self.scope_stack.enter_scope(block_expr);
        for stmt in block_expr.stmts.iter_mut() {
            self.visit_stmt(stmt)?;
        }

        let result = Ok(if let Some(expr) = &mut block_expr.last_expr {
            let is_none = dest.is_none();
            let res = self.visit_expr(&mut *expr, dest)?;
            if is_none && !res.is_unit_or_never() {
                return Err(format!(
                    "error in visiting block expr: expected `()`, found {:?}",
                    res
                )
                .into());
            }
            res
        } else {
            Operand::Unit
        });
        self.scope_stack.exit_scope();
        result
    }

    fn visit_assign_expr(&mut self, assign_expr: &mut AssignExpr) -> Result<Operand, RccError> {
        let operand = self.visit_lhs_expr(&mut assign_expr.lhs)?;
        let p = match operand {
            Operand::Place(p) => p,
            _ => unimplemented!(),
        };
        let type_info = assign_expr.lhs.type_info();
        let tp = type_info.borrow();
        let tp = tp.deref();
        let t = IRType::from_type_info(tp)?;

        macro_rules! add_inst {
            ($bin_op:path) => {{
                let rhs_dest = self.gen_temp_variable(assign_expr.lhs.type_info());
                let rhs = self.visit_expr(&mut assign_expr.rhs, Some(rhs_dest))?;
                self.ir_output.add_instructions(IRInst::bin_op(
                    $bin_op,
                    t,
                    p.clone(),
                    Operand::Place(p),
                    rhs.clone(),
                ))
            }};
        }
        match assign_expr.assign_op {
            AssignOp::Eq => {
                let rhs = self.visit_expr(&mut assign_expr.rhs, Some(p.clone()))?;
            }
            AssignOp::ShrEq => add_inst!(BinOperator::Shr),
            AssignOp::ShlEq => add_inst!(BinOperator::Shl),
            AssignOp::PlusEq => add_inst!(BinOperator::Plus),
            AssignOp::MinusEq => add_inst!(BinOperator::Minus),
            AssignOp::StarEq => add_inst!(BinOperator::Star),
            AssignOp::SlashEq => add_inst!(BinOperator::Slash),
            AssignOp::PercentEq => add_inst!(BinOperator::Percent),
            AssignOp::AndEq => add_inst!(BinOperator::And),
            AssignOp::OrEq => add_inst!(BinOperator::Or),
            AssignOp::CaretEq => add_inst!(BinOperator::Caret),
        }
        Ok(Operand::Unit)
    }

    fn visit_range_expr(&mut self, range_expr: &mut RangeExpr) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn bin_op(
        &mut self,
        lhs: Operand,
        rhs: Operand,
        op: BinOperator,
        ir_type: IRType,
        dest: Place,
    ) -> Result<Operand, RccError> {
        self.ir_output
            .add_instructions(IRInst::bin_op(op, ir_type, dest.clone(), lhs, rhs));
        Ok(Operand::Place(dest))
    }

    fn visit_bin_op_expr(
        &mut self,
        bin_op_expr: &mut BinOpExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        let d = self.gen_temp_variable(bin_op_expr.lhs.type_info());
        let lhs = self.visit_expr(&mut bin_op_expr.lhs, Some(d))?;
        let d = self.gen_temp_variable(bin_op_expr.rhs.type_info());
        let rhs = self.visit_expr(&mut bin_op_expr.rhs, Some(d))?;

        // TODO operator override
        let _type = bin_op_expr.type_info();
        let t = _type.borrow();
        let tp = t.deref();
        let ir_type = IRType::from_type_info(tp)?;

        let fold_option = bin_op_may_constant_fold(&bin_op_expr.bin_op, &lhs, &rhs)?;

        match dest {
            Some(d) => match self.optimize_level {
                OptimizeLevel::Zero => {
                    self.bin_op(lhs, rhs, bin_op_expr.bin_op, ir_type, d.clone())
                }

                OptimizeLevel::One => match fold_option {
                    Some(operand) => self.lit(operand, d),
                    None => self.bin_op(lhs, rhs, bin_op_expr.bin_op, ir_type, d.clone()),
                },
            },
            None => Ok(Operand::Unit),
        }
    }

    /// ## Example1
    ///
    /// let a = A() && B() || C() || D();
    ///
    /// <=>
    /// (1) a_0 = A()
    /// (2) if not a_0 goto (6)
    /// (3) a_0 = B()
    /// (4) if not a_0 goto (6)
    /// (5) goto ()
    /// (6) a_0 = C()
    /// (7) if a_0 goto ()
    /// (8) a_0 = D()
    /// (9) if a_0 goto ()
    /// a_0 = C()
    /// if a_0 goto LABEL
    /// a_0 = D()
    /// if a_0 goto LABEL
    /// LABEL:
    /// ...
    ///
    /// ## Example2
    ///
    /// if A() && B() || C() && (D() || E()) {
    ///     ...
    /// }
    ///
    /// <=>
    ///
    /// (1) if not A() goto (4)
    /// (2) if not B() goto (4)
    /// (3) goto (7)
    /// (4) if not C() goto ()
    /// (5) if D() goto (7)
    /// (6) if E() goto (7)
    /// (7) ... // do something
    /// (8) ...
    fn visit_logic_bin_expr(
        &mut self,
        bin_op_expr: &mut BinOpExpr,
        dest: Place,
    ) -> Result<Operand, RccError> {
        debug_assert!(matches!(
            bin_op_expr.bin_op,
            BinOperator::AndAnd | BinOperator::OrOr
        ));
        // let lhs = self.visit_expr(&mut bin_op_expr.lhs, dest)?;
        // let if_inst = if bin_op_expr.bin_op == BinOperator::AndAnd {
        //     IRInst::jump_if_not(lhs)
        // } else {
        //     IRInst::jump_if(lhs)
        // };
        // let if_idx = self.ir_output.instructions.len();
        // self.ir_output.add_instructions(if_inst);
        // let rhs = self.visit_expr(
        //     &mut bin_op_expr.rhs,
        //     dest.clone(),
        // );
        todo!()
    }

    fn visit_array_expr(&mut self, array_expr: &mut ArrayExpr) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_array_index_expr(
        &mut self,
        array_index_expr: &mut ArrayIndexExpr,
    ) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_tuple_expr(&mut self, tuple_expr: &mut TupleExpr) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_tuple_index_expr(
        &mut self,
        tuple_index_expr: &mut TupleIndexExpr,
    ) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_struct_expr(&mut self, struct_expr: &mut StructExpr) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_call_expr(
        &mut self,
        call_expr: &mut CallExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        let callee_place = self.gen_temp_variable(call_expr.type_info());
        let callee = self.visit_expr(&mut call_expr.expr, Some(callee_place))?;

        let mut params = vec![];
        for e in call_expr.call_params.iter_mut() {
            let param_place = self.gen_temp_variable(e.type_info());
            params.push(self.visit_expr(e, Some(param_place))?);
        }
        self.ir_output
            .add_instructions(IRInst::call(callee, params));
        match dest {
            Some(d) => {
                self.ir_output
                    .add_instructions(IRInst::load_data(d.clone(), Operand::FnRetPlace));
                Ok(Operand::Place(d))
            }
            None => Ok(Operand::Unit),
        }
    }

    fn visit_field_access_expr(
        &mut self,
        field_access_expr: &mut FieldAccessExpr,
    ) -> Result<Operand, RccError> {
        unimplemented!()
    }

    fn visit_loop_block(
        &mut self,
        loop_block: &mut BlockExpr,
        loop_start_id: usize,
    ) -> Result<(), RccError> {
        let operand = self.visit_block_expr(loop_block, None)?;
        assert!(operand.is_unit_or_never());
        self.ir_output.add_instructions(IRInst::jump(loop_start_id));
        let (d, mut link) = self.loop_var_stack.pop().unwrap();
        let next_id = self.ir_output.next_inst_id();
        while link != 0 {
            let inst = self.ir_output.get_inst_by_id(link);
            link = inst.jump_label();
            inst.set_jump_label(next_id);
        }
        Ok(())
    }

    /// While Expr always values ()
    fn visit_while_expr(&mut self, while_expr: &mut WhileExpr) -> Result<Operand, RccError> {
        let loop_start_id = self.ir_output.next_inst_id();

        let mut next_back_patch_link = 0;
        // while condition
        match while_expr.0.as_mut() {
            Expr::BinOp(e) => match e.bin_op {
                BinOperator::AndAnd => {
                    todo!()
                }
                BinOperator::OrOr => {
                    todo!()
                }
                BinOperator::Ne => {
                    self.gen_jump_cond(e, JEq, &mut next_back_patch_link)?;
                }
                BinOperator::EqEq => {
                    self.gen_jump_cond(e, JNe, &mut next_back_patch_link)?;
                }
                BinOperator::Le => {
                    self.gen_jump_cond_reverse(e, JLt, &mut next_back_patch_link)?;
                }
                BinOperator::Lt => {
                    self.gen_jump_cond(e, JGe, &mut next_back_patch_link)?;
                }
                BinOperator::Gt => {
                    self.gen_jump_cond_reverse(e, JGe, &mut next_back_patch_link)?;
                }
                BinOperator::Ge => {
                    self.gen_jump_cond(e, JLt, &mut next_back_patch_link)?;
                }
                _ => {
                    let d = self.gen_temp_variable(e.type_info());
                    let operand = self.visit_bin_op_expr(e, Some(d))?;

                    next_back_patch_link = self.ir_output.next_inst_id();
                    let ir_inst = IRInst::jump_if_not(operand, 0);
                    self.ir_output.add_instructions(ir_inst);
                }
            },
            // todo: unary expr, lit bool
            e => {
                let d = self.gen_temp_variable(e.type_info());
                let operand = self.visit_expr(e, Some(d))?;

                next_back_patch_link = self.ir_output.next_inst_id();
                let ir_inst = IRInst::jump_if_not(operand, 0);
                self.ir_output.add_instructions(ir_inst);
            }
        }
        self.loop_var_stack.push((None, next_back_patch_link));
        self.visit_loop_block(&mut while_expr.1, loop_start_id)?;
        Ok(Operand::Unit)
    }

    fn visit_loop_expr(
        &mut self,
        loop_expr: &mut LoopExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        let loop_start_id = self.ir_output.next_inst_id();
        self.loop_var_stack.push((dest.clone(), 0));
        self.visit_loop_block(&mut loop_expr.expr, loop_start_id)?;
        match dest {
            Some(p) => Ok(Operand::Place(p)),
            None => Ok(Operand::Never),
        }
    }

    /// ## Examples for translating `if` and logical condition expressions
    ///
    /// ### Primitive condition
    /// if A {
    ///     ... // BLOCK
    /// }
    /// ... // NEXT
    ///
    /// if not A goto NEXT
    ///
    /// ### Comparison condition
    /// if A > B {
    ///     ...
    /// }
    ///
    /// if not A > B goto NEXT
    ///
    /// ### Logical and condition
    /// if A && B {
    ///     ...
    /// }
    ///
    /// if not A goto NEXT
    /// if not B goto NEXT
    ///
    /// a = A() && B()
    ///
    /// if not A() goto NEXT
    /// a = B()
    ///
    /// ### Logical or condition
    ///
    /// if A || B {
    ///     ...
    /// }
    ///
    /// if A goto BLOCK
    /// if not B goto NEXT
    ///
    /// a = A() || B()
    ///
    /// if A() goto NEXT
    /// a = B()
    ///
    /// ### Comprehensive examples
    ///
    /// if A && B || C {
    ///     ...
    /// }
    ///
    /// if not A goto C
    /// if B goto BLOCK
    /// if not C goto NEXT
    ///
    /// if (A || B) && C || D && (E || F || G) {
    ///     ...
    /// }
    ///
    /// if A goto C
    /// if not B goto D
    ///                 -- ||1: left hit = C, right miss = D
    /// if C goto BLOCK
    ///                 -- &&1: left miss = D, right hit = BLOCK
    /// if not D goto NEXT
    ///                 -- ||2: left hit = BLOCK, right miss = NEXT
    /// if E goto BLOCK
    /// if F goto BLOCK
    ///                 -- ||3: left hit = BLOCK, right hit = BLOCK
    /// if not G goto NEXT
    ///                 -- ||4: left hit = BLOCK, right miss = NEXT
    ///
    /// if A || B && C {
    ///     ...
    /// }
    ///
    /// if A goto BLOCK
    /// if not B goto NEXT
    /// if not C goto NEXT
    ///
    /// if A && B && C {
    ///     ...
    /// }
    ///
    /// if not A goto NEXT
    /// if not B goto NEXT
    /// if not C goto NEXT
    ///
    /// Sequence (&&1, ||1, &&2, ||2, ||3, ... ||m, &&n)
    ///
    /// &&n left hit = &&n right next
    ///
    /// &&n ||m:
    ///     &&n left miss := ||m right
    ///     &&n right hit := ||m next
    ///     &&n right miss := &&n right next
    ///
    /// &&n &&n+1:
    ///     &&n left miss := &&n+1 right next
    ///     &&n right hit := &&n+1 next
    fn visit_if_expr(
        &mut self,
        if_expr: &mut IfExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        let mut next_back_patch_link = 0usize;

        macro_rules! visit_block {
            ($i:ident, $ir_inst:ident) => {
                self.visit_block_expr(if_expr.blocks.get_mut($i).unwrap(), dest.clone())?;
                if $i != if_expr.blocks.len() - 1 {
                    self.ir_output
                        .add_instructions(IRInst::jump(next_back_patch_link));
                    next_back_patch_link = self.ir_output.next_inst_id() - 1;
                }
            };
        };

        for (i, cond) in if_expr.conditions.iter_mut().enumerate() {
            match cond {
                Expr::BinOp(e) => match e.bin_op {
                    BinOperator::AndAnd => {
                        todo!()
                    }
                    BinOperator::OrOr => {
                        todo!()
                    }
                    BinOperator::Ne => {
                        self.gen_jump_cond(e, JEq, &mut next_back_patch_link)?;
                        visit_block!(i, ir_inst);
                    }
                    BinOperator::EqEq => {
                        self.gen_jump_cond(e, JNe, &mut next_back_patch_link)?;
                        visit_block!(i, ir_inst);
                    }
                    BinOperator::Le => {
                        self.gen_jump_cond_reverse(e, JLt, &mut next_back_patch_link)?;
                        visit_block!(i, ir_inst);
                    }
                    BinOperator::Lt => {
                        self.gen_jump_cond(e, JGe, &mut next_back_patch_link)?;
                        visit_block!(i, ir_inst);
                    }
                    BinOperator::Gt => {
                        self.gen_jump_cond_reverse(e, JGe, &mut next_back_patch_link)?;
                        visit_block!(i, ir_inst);
                    }
                    BinOperator::Ge => {
                        self.gen_jump_cond(e, JLt, &mut next_back_patch_link)?;
                        visit_block!(i, ir_inst);
                    }
                    _ => {
                        let d = self.gen_temp_variable(e.type_info());
                        let operand = self.visit_bin_op_expr(e, Some(d))?;
                        let ir_inst = IRInst::jump_if_not(operand, next_back_patch_link);
                        next_back_patch_link = self.ir_output.next_inst_id();
                        self.ir_output.add_instructions(ir_inst);
                        visit_block!(i, ir_inst);
                    }
                },
                // todo: unary expr, lit bool
                e => {
                    let d = self.gen_temp_variable(e.type_info());
                    let operand = self.visit_expr(e, Some(d))?;
                    let ir_inst = IRInst::jump_if_not(operand, next_back_patch_link);
                    next_back_patch_link = self.ir_output.next_inst_id();
                    self.ir_output.add_instructions(ir_inst);
                    visit_block!(i, ir_inst);
                }
            }
        }
        if if_expr.blocks.len() == if_expr.conditions.len() + 1 {
            self.visit_block_expr(if_expr.blocks.last_mut().unwrap(), dest.clone())?;
        }
        let next_idx = self.ir_output.next_inst_id();
        while next_back_patch_link != 0 {
            let inst = self.ir_output.get_inst_by_id(next_back_patch_link);
            next_back_patch_link = inst.jump_label();
            inst.set_jump_label(next_idx);
        }
        match dest {
            Some(d) => Ok(Operand::Place(d)),
            None => Ok(Operand::Unit),
        }
    }

    fn gen_jump_cond(
        &mut self,
        e: &mut BinOpExpr,
        jump: Jump,
        next_back_patch_link: &mut usize,
    ) -> Result<(), RccError> {
        let d = self.gen_temp_variable(e.type_info());
        let lhs = self.visit_expr(&mut e.lhs, Some(d))?;
        let d = self.gen_temp_variable(e.type_info());
        let rhs = self.visit_expr(&mut e.rhs, Some(d))?;
        let ir_inst = IRInst::jump_if_cond(jump, lhs, rhs, *next_back_patch_link);
        *next_back_patch_link = self.ir_output.next_inst_id();
        self.ir_output.add_instructions(ir_inst);
        Ok(())
    }

    fn gen_jump_cond_reverse(
        &mut self,
        e: &mut BinOpExpr,
        jump: Jump,
        next_back_patch_link: &mut usize,
    ) -> Result<(), RccError> {
        let d = self.gen_temp_variable(e.type_info());
        let lhs = self.visit_expr(&mut e.lhs, Some(d))?;
        let d = self.gen_temp_variable(e.type_info());
        let rhs = self.visit_expr(&mut e.rhs, Some(d))?;
        let ir_inst = IRInst::jump_if_cond(jump, rhs, lhs, *next_back_patch_link);
        *next_back_patch_link = self.ir_output.next_inst_id();
        self.ir_output.add_instructions(ir_inst);
        Ok(())
    }

    fn visit_return_expr(
        &mut self,
        return_expr: &mut ReturnExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        match &mut return_expr.0 {
            Some(e) => {
                let ret_place = self.fn_ret_temp_var.last().unwrap();
                let operand = self.visit_expr(e.as_mut(), Some(ret_place.clone()))?;
                self.ir_output.add_instructions(IRInst::Ret(operand));
            }
            None => {
                self.ir_output.add_instructions(IRInst::Ret(Operand::Unit));
            }
        };
        match dest {
            Some(d) => {
                self.ir_output
                    .add_instructions(IRInst::load_data(d.clone(), Operand::Never));
                Ok(Operand::Place(d))
            }
            None => Ok(Operand::Never),
        }
    }

    fn visit_break_expr(
        &mut self,
        break_expr: &mut BreakExpr,
        dest: Option<Place>,
    ) -> Result<Operand, RccError> {
        let (break_place, _) = self.loop_var_stack.last_mut().unwrap();
        match &mut break_expr.0 {
            Some(e) => {
                if let Some(p) = break_place {
                    let p = p.clone();
                    let temp_v = self.gen_temp_variable(e.type_info());
                    let rhs = self.visit_expr(e, Some(temp_v))?;
                    self.ir_output.add_instructions(IRInst::load_data(p, rhs));
                } else {
                    unreachable!("error in ir_builder: break expr has ret value");
                }
            }
            None => {
                if break_place.is_some() {
                    unreachable!("error in ir_builder: break expr shouldn't follow expr")
                }
            }
        }
        let jump_id = self.ir_output.next_inst_id();

        let (_, loop_start_id) = self.loop_var_stack.last_mut().unwrap();
        self.ir_output
            .add_instructions(IRInst::jump(*loop_start_id));
        *loop_start_id = jump_id;

        match dest {
            Some(d) => {
                self.ir_output
                    .add_instructions(IRInst::load_data(d.clone(), Operand::Never));
                Ok(Operand::Place(d))
            }
            None => Ok(Operand::Never),
        }
    }
}

/// Constant fold optimization.
/// a = 2 * 3 -> a = 6
/// TODO other primitive type
pub(super) fn bin_op_may_constant_fold(
    op: &BinOperator,
    src1: &Operand,
    src2: &Operand,
) -> Result<Option<Operand>, RccError> {
    Ok(match (src1, src2) {
        (Operand::I32(l), Operand::I32(r)) => match op {
            BinOperator::Plus => Some(Operand::I32(match l.checked_add(*r) {
                Some(res) => res,
                None => return Err("add overflow".into()),
            })),
            BinOperator::Minus => Some(Operand::I32(match l.checked_sub(*r) {
                Some(res) => res,
                None => return Err("sub overflow".into()),
            })),
            BinOperator::Star => Some(Operand::I32(match l.checked_mul(*r) {
                Some(res) => res,
                None => return Err("mul overflow".into()),
            })),
            BinOperator::Slash => Some(Operand::I32(match l.checked_div(*r) {
                Some(res) => res,
                None => return Err("div overflow".into()),
            })),
            BinOperator::Lt => Some(Operand::Bool(l < r)),
            BinOperator::Le => Some(Operand::Bool(l <= r)),
            BinOperator::Gt => Some(Operand::Bool(l > r)),
            BinOperator::Ge => Some(Operand::Bool(l >= r)),
            BinOperator::Ne => Some(Operand::Bool(l != r)),
            BinOperator::EqEq => Some(Operand::Bool(l == r)),
            BinOperator::Shl => Some(Operand::I32(match l.checked_shl(*r as u32) {
                Some(res) => res,
                None => return Err("shl overflow".into()),
            })),
            BinOperator::Shr => Some(Operand::I32(match l.checked_shr(*r as u32) {
                Some(res) => res,
                None => return Err("shr overflow".into()),
            })),
            BinOperator::And => Some(Operand::I32(l & r)),
            BinOperator::Or => Some(Operand::I32(l | r)),
            BinOperator::Caret => Some(Operand::I32(l ^ r)),
            BinOperator::Percent => Some(Operand::I32(match l.checked_rem(*r) {
                Some(res) => res,
                None => return Err("rem overflow".into()),
            })),
            _ => None,
        },
        _ => None,
    })
}
