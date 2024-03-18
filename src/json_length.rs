/// Find when a JSON value ends in a string
pub fn json_length(string: &str) -> Option<usize> {
    // FIXME: Handle non-objects and nested objects
    // For now, we only need to find '}'
    string.find("}").map(|index| index + 1)
}