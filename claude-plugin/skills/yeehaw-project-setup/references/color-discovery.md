# Color Discovery Guide

Strategies for finding or deriving an appropriate brand color for a project.

## Priority Order

1. **Explicit brand colors** - Defined brand/theme colors in config
2. **Primary UI colors** - Main colors used in the interface
3. **Framework/tech association** - Colors associated with the primary technology
4. **Derived aesthetically** - Based on project type/domain

---

## 1. Explicit Brand Colors

### Tailwind Config
```
tailwind.config.js
tailwind.config.ts
```
Look for:
- `theme.extend.colors.primary`
- `theme.extend.colors.brand`
- Custom color definitions

### CSS Variables
```
src/styles/variables.css
src/styles/globals.css
src/index.css
app/globals.css
styles/theme.css
```
Look for:
- `--primary-color`
- `--brand-color`
- `--color-primary`
- `--accent-color`

### Theme Configuration
```
theme.json
theme.config.js
src/theme/*
```

### Design Tokens
```
tokens.json
design-tokens.json
src/tokens/*
```

### Package.json
Some projects define theme colors:
```json
{
  "theme": {
    "primaryColor": "#..."
  }
}
```

---

## 2. UI Framework Colors

### Material UI / MUI
```
src/theme.ts
src/theme/index.ts
```
Look for `palette.primary.main`

### Chakra UI
```
src/theme/index.ts
theme.ts
```
Look for `colors.brand` or `colors.primary`

### Ant Design
```
.antd-theme.json
config-overrides.js
```
Look for `@primary-color`

### Bootstrap
```
scss/_variables.scss
src/styles/variables.scss
```
Look for `$primary`

---

## 3. Asset-Based Discovery

### Logos and Icons
```
public/logo.*
public/favicon.*
src/assets/logo.*
assets/brand/*
static/logo.*
```
If an SVG logo exists, extract dominant color from fill/stroke attributes.

### App Icons (Mobile)
```
ios/*/Images.xcassets/AppIcon.appiconset
android/app/src/main/res/mipmap-*/ic_launcher.png
```

---

## 4. Framework/Technology Colors

When no brand color is found, consider the primary technology:

| Technology | Suggested Color | Hex |
|------------|----------------|-----|
| React | React Blue | #61DAFB |
| Vue | Vue Green | #42B883 |
| Angular | Angular Red | #DD0031 |
| Svelte | Svelte Orange | #FF3E00 |
| Next.js | Black | #000000 |
| Node.js | Node Green | #339933 |
| Python | Python Blue | #3776AB |
| Ruby/Rails | Ruby Red | #CC342D |
| Go | Go Blue | #00ADD8 |
| Rust | Rust Orange | #DEA584 |
| Laravel | Laravel Red | #FF2D20 |
| Django | Django Green | #092E20 |
| Spring | Spring Green | #6DB33F |
| .NET | .NET Purple | #512BD4 |
| PHP | PHP Purple | #777BB4 |
| Elixir | Elixir Purple | #4B275F |

---

## 5. Domain-Based Colors

When technology doesn't suggest a color, consider the domain:

| Domain | Color Family | Example Hex |
|--------|--------------|-------------|
| Finance/Banking | Blue, Navy | #1A365D |
| Healthcare | Blue, Teal | #0D9488 |
| E-commerce | Orange, Purple | #F97316 |
| Education | Blue, Green | #2563EB |
| Social | Blue, Pink | #3B82F6 |
| Entertainment | Purple, Red | #7C3AED |
| Productivity | Blue, Gray | #475569 |
| Developer Tools | Dark, Purple | #6366F1 |
| Food/Restaurant | Orange, Red | #EA580C |
| Travel | Blue, Teal | #0891B2 |
| Real Estate | Blue, Green | #059669 |
| Sports/Fitness | Orange, Red | #DC2626 |

---

## Color Format

Return colors as 6-character hex codes with leading `#`:
- Correct: `#FF6B6B`
- Incorrect: `FF6B6B`, `#F66`, `rgb(255, 107, 107)`

Ensure sufficient contrast - avoid colors that are too light (hard to see) or too dark (looks black). Ideal lightness range: 30-70% in HSL.
