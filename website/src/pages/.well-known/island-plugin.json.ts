import document from '../../../plugin.json';
export function GET() {
  return new Response(JSON.stringify(document, null, 2), {
    headers: { 'Content-Type': 'application/json; charset=utf-8' },
  });
}
