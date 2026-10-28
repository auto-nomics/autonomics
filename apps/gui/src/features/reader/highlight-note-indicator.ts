type CommentedHighlight = { id: string; comment?: string; color?: string };

/** One note affordance per commented highlight, separate from painted text
 * rectangles so it does not enlarge the selected annotation's outline. */
export function highlightNoteIndicator(
  annotation: CommentedHighlight,
  select: (id: string) => void,
): HTMLButtonElement | null {
  const comment = annotation.comment?.trim();
  if (!comment) return null;
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'reader-note-marker reader-highlight-note-indicator';
  button.dataset.annotationId = annotation.id;
  button.title = comment;
  button.setAttribute('aria-label', 'Open highlight note');
  button.style.background = annotation.color ?? '#ffd400';
  button.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-9M12 12l8-8M16 3l5 5"/></svg>';
  // Clicking a note must not collapse/change an unrelated native text selection
  // before the inspector explicitly takes ownership of the annotation.
  button.addEventListener('pointerdown', event => { event.preventDefault(); event.stopPropagation(); });
  button.addEventListener('click', event => { event.stopPropagation(); select(annotation.id); });
  return button;
}
