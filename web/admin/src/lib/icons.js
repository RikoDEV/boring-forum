// The site's icon sprites, for the panel's icon pickers (IconPicker.svelte, which wraps the
// site's own picker in web/islands/src/icon-picker.js).
import sprite from "../../../../themes/default/icons.svg?url";
import spriteText from "../../../../themes/default/icons.svg?raw";
import brandSprite from "../../../../themes/default/brands.svg?url";
import brandNames from "../../../../themes/default/brand-names.json";
import { iconList } from "../../../islands/src/icon-picker.js";

export { sprite, brandNames };

/** Every general icon a group, field, forum or badge may use, in sprite order. */
export const ICONS = iconList(spriteText);

/** Brand logos (`brand-*`), A to Z by name, from their own sprite. */
export const BRANDS = Object.keys(brandNames).sort((a, b) => brandNames[a].localeCompare(brandNames[b]));

/** Where an icon is drawn from: brands have a sprite of their own, as on the site. */
export const iconHref = (name) => `${name.startsWith("brand-") ? brandSprite : sprite}#${name}`;

/** A readable name: "GitHub", or "map pin". */
export const iconName = (name) => brandNames[name] ?? name.replaceAll("-", " ");
