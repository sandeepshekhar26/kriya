"""Parameter validation parity with kriya-core's validate.ts."""

import unittest

from kriya import ParameterSchema, array, boolean, number, obj, required, string
from kriya.validate import format_issues, validate_params


class TestValidate(unittest.TestCase):
    def test_accepts_correct_types(self):
        schemas = {"title": string, "n": number, "flag": boolean}
        issues = validate_params({"title": "hi", "n": 3, "flag": True}, schemas)
        self.assertEqual(issues, [])

    def test_wrong_type_reports_expected_and_actual(self):
        issues = validate_params({"title": 5}, {"title": string})
        self.assertEqual(len(issues), 1)
        self.assertEqual(issues[0].path, "title")
        self.assertEqual(issues[0].message, "expected string, got number")

    def test_bool_is_not_a_number(self):
        # bool is a subclass of int in Python; must classify as boolean, not number.
        issues = validate_params({"n": True}, {"n": number})
        self.assertEqual(issues[0].message, "expected number, got boolean")

    def test_required_missing(self):
        issues = validate_params({}, {"title": required(string)})
        self.assertEqual(issues[0].path, "title")
        self.assertEqual(issues[0].message, "required")

    def test_optional_missing_is_fine(self):
        self.assertEqual(validate_params({}, {"title": string}), [])

    def test_present_none_is_a_type_error_not_missing(self):
        issues = validate_params({"title": None}, {"title": required(string)})
        self.assertEqual(issues[0].message, "expected string, got null")

    def test_enum(self):
        sch = {"color": ParameterSchema("string", enum=("red", "green"))}
        self.assertEqual(validate_params({"color": "red"}, sch), [])
        bad = validate_params({"color": "blue"}, sch)
        self.assertIn("is not one of", bad[0].message)

    def test_array_element_types(self):
        sch = {"tags": array("string")}
        self.assertEqual(validate_params({"tags": ["a", "b"]}, sch), [])
        bad = validate_params({"tags": ["a", 2]}, sch)
        self.assertEqual(bad[0].path, "tags[1]")
        self.assertEqual(bad[0].message, "expected string, got number")

    def test_nested_object_required(self):
        sch = {"meta": obj({"id": required(string)})}
        bad = validate_params({"meta": {}}, sch)
        self.assertEqual(bad[0].path, "meta.id")
        self.assertEqual(bad[0].message, "required")

    def test_unknown_params_ignored(self):
        self.assertEqual(validate_params({"extra": 1}, {"title": string}), [])

    def test_format_issues(self):
        issues = validate_params({"title": 5}, {"title": string, "n": required(number)})
        self.assertEqual(
            format_issues(issues), "title: expected string, got number; n: required"
        )


if __name__ == "__main__":
    unittest.main()
