(module
  (func (export "add") (param $left i32) (param $right i32) (result i32)
    local.get $left
    local.get $right
    i32.add))
