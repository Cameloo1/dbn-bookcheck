import eslint from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: ["dist/**", "node_modules/**", "outputs/**"],
  },
  {
    languageOptions: {
      globals: {
        AbortController: "readonly",
        console: "readonly",
        document: "readonly",
        fetch: "readonly",
        process: "readonly",
        URL: "readonly",
        window: "readonly",
      },
    },
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
    },
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
);
