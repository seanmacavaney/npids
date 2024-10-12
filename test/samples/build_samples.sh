ir_datasets export beir/dbpedia-entity docs --fields doc_id | gzip > beir_dbpedia-entity.txt.gz
ir_datasets export beir/fever docs --fields doc_id | gzip > beir_fever.txt.gz
ir_datasets export cord19 docs --fields doc_id | gzip > cord19.txt.gz
ir_datasets export disks45/nocr docs --fields doc_id | gzip > disks45_nocr.txt.gz
ir_datasets export msmarco-document docs --fields doc_id | gzip > msmarco-document.txt.gz
ir_datasets export msmarco-passage docs --fields doc_id | gzip > msmarco-passage.txt.gz
ir_datasets export neuclir/1/fa docs --fields doc_id | gzip > neuclir_1_fa.txt.gz
ir_datasets export vaswani docs --fields doc_id | gzip > vaswani.txt.gz
