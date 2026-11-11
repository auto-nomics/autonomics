import { createRoot } from 'react-dom/client';
import { RssSubscriptions } from '../src/features/discovery/rss';
export function mount() { createRoot(document.body).render(<div style={{height:'100vh',display:'flex'}}><RssSubscriptions /></div>); }
