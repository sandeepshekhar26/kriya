"""JSON Schema export parity with kriya-core's jsonschema.ts (required lifted to an array, etc.)."""

import unittest

from kriya import array, number, obj, params_to_json_schema, required, string


class TestJsonSchema(unittest.TestCase):
    def test_lifts_required_to_object_array(self):
        schema = params_to_json_schema(
            {"title": required(string), "body": string}
        )
        self.assertEqual(
            schema,
            {
                "type": "object",
                "properties": {"title": {"type": "string"}, "body": {"type": "string"}},
                "required": ["title"],
            },
        )

    def test_no_required_key_when_none_required(self):
        schema = params_to_json_schema({"body": string})
        self.assertNotIn("required", schema)

    def test_per_property_required_boolean_never_leaks(self):
        # strict validators reject a per-property `required: true`; it must only appear as an array.
        schema = params_to_json_schema({"title": required(string)})
        self.assertNotIn("required", schema["properties"]["title"])

    def test_array_items_string_expanded_to_schema_object(self):
        schema = params_to_json_schema({"tags": array("string")})
        self.assertEqual(
            schema["properties"]["tags"], {"type": "array", "items": {"type": "string"}}
        )

    def test_nested_object_required_array(self):
        schema = params_to_json_schema(
            {"meta": obj({"id": required(string), "n": number})}
        )
        meta = schema["properties"]["meta"]
        self.assertEqual(meta["type"], "object")
        self.assertEqual(meta["required"], ["id"])
        self.assertEqual(meta["properties"]["id"], {"type": "string"})

    def test_description_and_enum_forwarded(self):
        from kriya import ParameterSchema

        schema = params_to_json_schema(
            {"c": ParameterSchema("string", description="a color", enum=("r", "g"))}
        )
        self.assertEqual(
            schema["properties"]["c"],
            {"type": "string", "description": "a color", "enum": ["r", "g"]},
        )


if __name__ == "__main__":
    unittest.main()
