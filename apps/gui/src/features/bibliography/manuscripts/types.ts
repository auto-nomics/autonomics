export type Project = {
  id: string;
  title: string;
  main_file: string;
  article_ids: string[];
  created_at: string;
  updated_at: string;
};
export type SourceFile = {
  path: string;
  content: string;
  revision: number;
  updated_at: string;
};
export type Proposal = {
  id: string;
  path: string;
  base_revision: number;
  original: string;
  proposed: string;
  status: string;
  author: string;
  summary: string;
  created_at: string;
};
export type Comment = {
  id: string;
  path: string;
  start_line: number;
  end_line: number;
  revision?: number;
  body: string;
  author: string;
  resolved: boolean;
  created_at: string;
};
export type Revision = {
  id: string;
  path: string;
  content: string;
  revision: number;
  actor: string;
  message: string;
  created_at: string;
};
export type Detail = {
  project: Project;
  files: SourceFile[];
  assets?: Array<{
    path: string;
    revision: number;
    byte_size: number;
    updated_at: string;
  }>;
  proposals: Proposal[];
  comments: Comment[];
};
export const projectPath = (id: string) =>
  `/bib/manuscripts/${encodeURIComponent(id)}`;
