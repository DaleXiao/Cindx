import reactHooks from "eslint-plugin-react-hooks";
import tsParser from "@typescript-eslint/parser";

// Narrow hooks-safety lint: the structural gate cannot see conditional hook
// ordering across every component, but rules-of-hooks can (audit P1-06).
export default [
  {
    files: ["src/**/*.{ts,tsx}", "tests/**/*.{ts,tsx}"],
    languageOptions: {
      parser: tsParser,
      ecmaVersion: "latest",
      sourceType: "module"
    },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error"
    }
  }
];
