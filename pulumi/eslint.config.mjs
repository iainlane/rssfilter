import eslint from "@eslint/js";
import eslintPluginPrettierRecommended from "eslint-plugin-prettier/recommended";
import unicornPlugin from "eslint-plugin-unicorn";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    plugins: {
      ["@typescript-eslint"]: tseslint.plugin,
      ["unicorn"]: unicornPlugin,
    },
  },

  eslint.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  ...tseslint.configs.stylisticTypeChecked,

  {
    languageOptions: {
      globals: {
        ...globals.es2020,
        ...globals.node,
      },

      parserOptions: {
        allowAutomaticSingleRunInference: true,
        project: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },

    rules: {
      "@typescript-eslint/consistent-indexed-object-style": [
        "error",
        "index-signature",
      ],
      "@typescript-eslint/dot-notation": "error",
      "@typescript-eslint/no-unsafe-enum-comparison": "off",
      "@typescript-eslint/no-unused-vars": ["off"],
      "@typescript-eslint/restrict-template-expressions": "off",
      "dot-notation": "off",

      "unicorn/no-typeof-undefined": "error",
    },
  },
  {
    files: ["**/*.mjs"],
    extends: [tseslint.configs.disableTypeChecked],
  },
  eslintPluginPrettierRecommended,
);
