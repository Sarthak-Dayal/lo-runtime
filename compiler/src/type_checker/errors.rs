//! Type-checker diagnostics and their stable external codes.

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub code: ErrorCode,
    pub line: u32,
    pub message: String,
}

impl TypeError {
    pub(super) fn new(code: ErrorCode, line: u32, message: impl Into<String>) -> Self {
        TypeError {
            code,
            line,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ErrorCode {
    // well-formedness
    EDuplicateClassName,
    EDuplicateField,
    EDuplicateMethod,
    EDuplicateConstructorArity,
    EFieldTypedVoid,
    EFormalTypedVoid,
    EReturnInVoidMethod,
    EReturnMissing,
    EReturnInConstructor,
    ELocalShadowsFormal,
    EBreakOutsideLoop,
    EWellFormednessOther,
    // name resolution
    EUnknownVariable,
    EReservedVariableName,
    EReservedClassName,
    EUnknownClass,
    EUnknownMethod,
    /// Never constructed: `Expr::This` only ever parses inside a `BodyScope`
    /// (a method or constructor body), so this AST has no position for
    /// `this` to appear "outside" one. Kept for 1:1 coverage of the
    /// reference vocabulary.
    EThisOutsideInstance,
    ENameResolutionOther,
    // type-check
    ETypeMismatch,
    EAssignTypeMismatch,
    EReturnTypeMismatch,
    EActualTypeMismatch,
    EBinopTypeMismatch,
    EUnopTypeMismatch,
    EConditionalTypeMismatch,
    EArityMismatch,
    EReceiverNotClassType,
    ENullLiteralReceiver,
    ENonvoidCallAsStatement,
    EVoidCallInExpression,
    ETypeCheckOther,
    // inheritance-check
    EInheritanceCycle,
    EFieldShadowing,
    EOverrideSignatureMismatch,
    EMissingConstructorInInheritingClass,
    ESuperInRootClass,
    ESuperMethodInRootClass,
    ESuperMethodUnresolved,
    EDelegationCycle,
    EDelegationArityMismatch,
    EInheritanceCheckOther,
    // cast / instanceof
    ECastTargetNotClass,
    ECastSourceNotClass,
    ECastUnrelatedTypes,
    EInstanceofSourceNotClass,
    ECastInstanceofOther,
    // entry point
    ENoMainClass,
    EMainClassExtends,
    ENoMainMethod,
    EMainMethodSignature,
    EMainNoZeroArgConstructor,
    EEntryPointOther,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        use ErrorCode::*;
        match self {
            EDuplicateClassName => "E_DUPLICATE_CLASS_NAME",
            EDuplicateField => "E_DUPLICATE_FIELD",
            EDuplicateMethod => "E_DUPLICATE_METHOD",
            EDuplicateConstructorArity => "E_DUPLICATE_CONSTRUCTOR_ARITY",
            EFieldTypedVoid => "E_FIELD_TYPED_VOID",
            EFormalTypedVoid => "E_FORMAL_TYPED_VOID",
            EReturnInVoidMethod => "E_RETURN_IN_VOID_METHOD",
            EReturnMissing => "E_RETURN_MISSING",
            EReturnInConstructor => "E_RETURN_IN_CONSTRUCTOR",
            ELocalShadowsFormal => "E_LOCAL_SHADOWS_FORMAL",
            EBreakOutsideLoop => "E_BREAK_OUTSIDE_LOOP",
            EWellFormednessOther => "E_WELL_FORMEDNESS_OTHER",
            EUnknownVariable => "E_UNKNOWN_VARIABLE",
            EReservedVariableName => "E_RESERVED_VARIABLE_NAME",
            EReservedClassName => "E_RESERVED_CLASS_NAME",
            EUnknownClass => "E_UNKNOWN_CLASS",
            EUnknownMethod => "E_UNKNOWN_METHOD",
            EThisOutsideInstance => "E_THIS_OUTSIDE_INSTANCE",
            ENameResolutionOther => "E_NAME_RESOLUTION_OTHER",
            ETypeMismatch => "E_TYPE_MISMATCH",
            EAssignTypeMismatch => "E_ASSIGN_TYPE_MISMATCH",
            EReturnTypeMismatch => "E_RETURN_TYPE_MISMATCH",
            EActualTypeMismatch => "E_ACTUAL_TYPE_MISMATCH",
            EBinopTypeMismatch => "E_BINOP_TYPE_MISMATCH",
            EUnopTypeMismatch => "E_UNOP_TYPE_MISMATCH",
            EConditionalTypeMismatch => "E_CONDITIONAL_TYPE_MISMATCH",
            EArityMismatch => "E_ARITY_MISMATCH",
            EReceiverNotClassType => "E_RECEIVER_NOT_CLASS_TYPE",
            ENullLiteralReceiver => "E_NULL_LITERAL_RECEIVER",
            ENonvoidCallAsStatement => "E_NONVOID_CALL_AS_STATEMENT",
            EVoidCallInExpression => "E_VOID_CALL_IN_EXPRESSION",
            ETypeCheckOther => "E_TYPE_CHECK_OTHER",
            EInheritanceCycle => "E_INHERITANCE_CYCLE",
            EFieldShadowing => "E_FIELD_SHADOWING",
            EOverrideSignatureMismatch => "E_OVERRIDE_SIGNATURE_MISMATCH",
            EMissingConstructorInInheritingClass => "E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS",
            ESuperInRootClass => "E_SUPER_IN_ROOT_CLASS",
            ESuperMethodInRootClass => "E_SUPER_METHOD_IN_ROOT_CLASS",
            ESuperMethodUnresolved => "E_SUPER_METHOD_UNRESOLVED",
            EDelegationCycle => "E_DELEGATION_CYCLE",
            EDelegationArityMismatch => "E_DELEGATION_ARITY_MISMATCH",
            EInheritanceCheckOther => "E_INHERITANCE_CHECK_OTHER",
            ECastTargetNotClass => "E_CAST_TARGET_NOT_CLASS",
            ECastSourceNotClass => "E_CAST_SOURCE_NOT_CLASS",
            ECastUnrelatedTypes => "E_CAST_UNRELATED_TYPES",
            EInstanceofSourceNotClass => "E_INSTANCEOF_SOURCE_NOT_CLASS",
            ECastInstanceofOther => "E_CAST_INSTANCEOF_OTHER",
            ENoMainClass => "E_NO_MAIN_CLASS",
            EMainClassExtends => "E_MAIN_CLASS_EXTENDS",
            ENoMainMethod => "E_NO_MAIN_METHOD",
            EMainMethodSignature => "E_MAIN_METHOD_SIGNATURE",
            EMainNoZeroArgConstructor => "E_MAIN_NO_ZERO_ARG_CONSTRUCTOR",
            EEntryPointOther => "E_ENTRY_POINT_OTHER",
        }
    }
}
