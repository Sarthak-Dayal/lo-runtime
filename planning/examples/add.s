.text
.globl add
.type add,@function
add:
    .functype add (i32, i32) -> (i32)
    local.get 0
    local.get 1
    i32.add
    end_function
