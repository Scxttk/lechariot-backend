-- Le Chariot – Migration v8: Filial-Koordinaten

alter table public.markets add column if not exists lat double precision;
alter table public.markets add column if not exists lon double precision;
