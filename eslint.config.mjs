import eslint from "@eslint/js";
import tseslint from "typescript-eslint";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";

export default tseslint.config(
  {
    ignores: ["**/dist/**", "**/.expo/**", "apps/mobile/android/**", "apps/mobile/ios/**"],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.mjs", "apps/server/**/*.ts", "packages/**/*.test.ts", "apps/mobile/**/*.test.ts"],
    languageOptions: { globals: globals.node },
  },
  {
    files: ["apps/{web,mobile}/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
    },
  },
  {
    files: ["apps/web/**/*.{ts,tsx}"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector: "Identifier[name='useEffect'], Literal[value='useEffect']",
          message: "Use event handlers, derived state or useSyncExternalStore in the web client.",
        },
      ],
    },
  },
);
