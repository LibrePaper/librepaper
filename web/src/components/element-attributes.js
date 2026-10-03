/**
 * Rebind Skeleton's slot attributes to the host element rendered by its
 * polymorphic `element` snippet. Skeleton's wrappers declare their slot
 * against the default host (usually a button), while Zag's runtime handlers
 * are generic DOM handlers and are intentionally used on anchors and tree
 * rows too. The runtime object is preserved whole, including Svelte's
 * attachment symbol; only its target-specific event type changes here.
 *
 * @template {Element} Source
 * @template {Element} Target
 * @param {import("svelte/elements").HTMLAttributes<Source>} attributes
 * @returns {import("svelte/elements").HTMLAttributes<Target>}
 */
export function retargetElementAttributes(attributes) {
  return /** @type {import("svelte/elements").HTMLAttributes<Target>} */ (
    /** @type {unknown} */ (attributes)
  );
}
