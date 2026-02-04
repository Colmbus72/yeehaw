import { execaSync } from 'execa';
import { writeFileSync, readFileSync, unlinkSync, existsSync } from 'fs';
import { join } from 'path';
import { tmpdir } from 'os';
import { randomBytes } from 'crypto';

/**
 * Open content in user's editor and return the edited content.
 * Similar to how git opens commit messages.
 */
export function editInEditor(initialContent: string, filename: string = 'edit.md'): string | null {
  const editor = process.env.EDITOR || process.env.VISUAL || 'nano';
  const tempFile = join(tmpdir(), `yeehaw-${randomBytes(4).toString('hex')}-${filename}`);

  try {
    // Write initial content to temp file
    writeFileSync(tempFile, initialContent, 'utf-8');

    // Open editor (this blocks until editor is closed)
    execaSync(editor, [tempFile], {
      stdio: 'inherit',
    });

    // Read back the edited content
    if (existsSync(tempFile)) {
      const content = readFileSync(tempFile, 'utf-8');
      unlinkSync(tempFile);
      return content;
    }
    return null;
  } catch (error) {
    // Clean up temp file if it exists
    if (existsSync(tempFile)) {
      unlinkSync(tempFile);
    }
    return null;
  }
}
