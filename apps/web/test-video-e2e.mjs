import { chromium } from 'playwright';

const NOTE_ID = 'e773051d-de5f-4613-a859-a736ab5c5c86';
const VIDEO_ID = '69bc8391-c8d1-4d3e-a0a5-8a9c02537cad';
const VIDEO_URL = `/api/notes/${NOTE_ID}/videos/${VIDEO_ID}`;
const BASE = 'http://localhost:5173';

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();

// Collect console errors
const errors = [];
page.on('console', msg => {
  if (msg.type() === 'error') errors.push(msg.text());
});

// 1. Navigate to notes page and select the note
console.log('--- Test 1: Navigate to app ---');
await page.goto(BASE);
await page.waitForTimeout(1000);

// Look for the note in sidebar and click it
const noteItem = await page.$(`[data-note-id="${NOTE_ID}"]`) 
  || await page.$(`text=CM6 Live Preview`);
if (noteItem) {
  await noteItem.click();
  await page.waitForTimeout(500);
  console.log('Note selected');
} else {
  // Try clicking through the notes page
  await page.goto(`${BASE}/#/notes`);
  await page.waitForTimeout(1000);
  console.log('Navigated to notes');
}

// 2. Check if the editor is loaded
console.log('--- Test 2: Check editor loaded ---');
const editorEl = await page.$('.cm-editor');
console.log('Editor found:', !!editorEl);

// 3. Test video content insertion - put the video markdown into the note content
console.log('--- Test 3: Inject video markdown into note ---');

// Click on the editor to focus it
if (editorEl) {
  await editorEl.click();
  await page.waitForTimeout(200);

  // Use keyboard to type video markdown at the end
  // First go to end of document
  await page.keyboard.press('End');
  await page.keyboard.press('End');
  await page.waitForTimeout(100);
  
  // Insert a new line with video markdown
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await page.keyboard.type(`![](${VIDEO_URL})`, { delay: 10 });
  await page.waitForTimeout(500);
  
  // Save
  await page.keyboard.press('Control+s');
  await page.waitForTimeout(1000);
  console.log('Video markdown inserted and saved');
}

// 4. Check if video widget is rendered (in non-active block)
console.log('--- Test 4: Check video rendering ---');
// Move cursor away from the video line to make it a non-active block
await page.keyboard.press('ArrowUp');
await page.keyboard.press('ArrowUp');
await page.waitForTimeout(300);

const videoWidget = await page.$('.cm-video-container video');
console.log('Video <video> element found:', !!videoWidget);

if (videoWidget) {
  const src = await videoWidget.getAttribute('src');
  console.log('Video src:', src);
  const controls = await videoWidget.getAttribute('controls');
  console.log('Controls attribute:', controls);
}

// 5. Check that video upload placeholder styles exist in CSS
console.log('--- Test 5: Check CSS ---');
const cssCheck = await page.evaluate(() => {
  // Check if the CSS classes are defined by creating a test element
  const div = document.createElement('div');
  div.className = 'cm-video-container';
  document.body.appendChild(div);
  const style = window.getComputedStyle(div);
  const result = { display: style.display, maxWidth: style.maxWidth };
  document.body.removeChild(div);
  return result;
});
console.log('CSS check:', cssCheck);

// 6. Test paste event handler
console.log('--- Test 6: Test video paste handler ---');
const pasteResult = await page.evaluate(() => {
  const editor = document.querySelector('.cm-editor .cm-content');
  if (!editor) return { error: 'No editor content' };
  
  // Simulate paste with a video file
  const videoFile = new File(['fake video content'], 'test.mp4', { type: 'video/mp4' });
  const dataTransfer = new DataTransfer();
  dataTransfer.items.add(videoFile);
  const clipboardEvent = new ClipboardEvent('paste', {
    bubbles: true,
    cancelable: true,
    clipboardData: dataTransfer
  });
  
  const dispatched = editor.dispatchEvent(clipboardEvent);
  return { dispatched, defaultPrevented: clipboardEvent.defaultPrevented };
});
console.log('Paste event test:', pasteResult);

// Summary
console.log('\n--- Summary ---');
if (errors.length > 0) {
  console.log('Console errors:', errors);
} else {
  console.log('No console errors');
}

// Take a screenshot
await page.screenshot({ path: '/tmp/video-e2e-screenshot.png', fullPage: true });
console.log('Screenshot saved to /tmp/video-e2e-screenshot.png');

await browser.close();
