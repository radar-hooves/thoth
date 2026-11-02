export default {
  extends: ['stylelint-config-standard', 'stylelint-config-html/svelte'],
  rules: {
    'color-no-hex': true,
    // Tailwind v4 at-rules
    'at-rule-no-unknown': [
      true,
      {
        ignoreAtRules: [
          'theme',
          'utility',
          'custom-variant',
          'plugin',
          'apply',
          'layer',
          'tailwind',
          // @source — the content-scan hint that makes Tailwind compile
          // @poodle64/ui's node_modules-hosted utility classes.
          'source',
        ],
      },
    ],
    // @apply takes utility class names, not CSS
    'at-rule-prelude-no-invalid': [true, { ignoreAtRules: ['apply'] }],
    // Allow shadcn/Tailwind class naming patterns
    'custom-property-pattern': null,
    'selector-class-pattern': null,
    'no-descending-specificity': null,
    // Formatting rules that conflict with prettier or the existing codebase
    'import-notation': null,
    'comment-empty-line-before': null,
    'declaration-empty-line-before': null,
    'custom-property-empty-line-before': null,
    'rule-empty-line-before': null,
    // oklch decimal notation (0.223 / 67.7) is valid CSS; prefer % and deg are style opinions
    'lightness-notation': null,
    'hue-degree-notation': null,
    // rgba() and decimal alpha are used in Svelte component style blocks; fine as-is
    'color-function-alias-notation': null,
    'color-function-notation': null,
    'alpha-value-notation': null,
    // -webkit-user-select is used for macOS drag-region support in Tauri
    'property-no-vendor-prefix': null,
    // :global() is Svelte's CSS scoping escape hatch; not unknown
    'selector-pseudo-class-no-unknown': [
      true,
      { ignorePseudoClasses: ['global'] },
    ],
    // Shorthand with trailing zeros is explicit and readable
    'shorthand-property-no-redundant-values': null,
    // break-word is used in existing Svelte style blocks
    'declaration-property-value-keyword-no-deprecated': null,
    // media-feature-range-notation: context is used in Svelte scoped styles
    'media-feature-range-notation': null,
    // Existing codebase uses camelCase keyframe names
    'keyframes-name-pattern': null,
    // Allow mixed keyword case (e.g. hsl values)
    'value-keyword-case': null,
  },
};
