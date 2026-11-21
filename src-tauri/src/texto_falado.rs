//! Ajustes de última hora no texto de cada trecho, só para a voz.
//!
//! O texto mostrado (legenda, destaque na página) continua o original: aqui se
//! mexe apenas no que vai para o sintetizador, para o destaque seguir casando
//! com o que está na tela.

pub fn para_a_voz(trecho: &str) -> String {
    let sem_muleta = reduzir_muleta(trecho);
    let com_unidades = expandir_unidades(&sem_muleta);
    fechar_frase(&com_unidades)
}

fn nucleo_da_palavra(palavra: &str) -> String {
    palavra
        .trim_matches(|caractere: char| !caractere.is_alphanumeric())
        .to_lowercase()
}

fn reduzir_muleta(trecho: &str) -> String {
    let mut mantidas: Vec<&str> = Vec::new();
    for palavra in trecho.split_whitespace() {
        let repetida = nucleo_da_palavra(palavra) == "né"
            && mantidas.last().is_some_and(|anterior| nucleo_da_palavra(anterior) == "né");
        if repetida {
            if let Some(ultima) = mantidas.last_mut() {
                *ultima = palavra;
            }
        } else {
            mantidas.push(palavra);
        }
    }
    mantidas.join(" ")
}

fn nome_da_unidade(sufixo: &str, singular: bool) -> Option<&'static str> {
    let nome = match (sufixo, singular) {
        ("ms", true) => "milissegundo",
        ("ms", false) => "milissegundos",
        ("s", true) => "segundo",
        ("s", false) => "segundos",
        ("min", true) => "minuto",
        ("min", false) => "minutos",
        ("h", true) => "hora",
        ("h", false) => "horas",
        _ => return None,
    };
    Some(nome)
}

fn expandir_unidades(texto: &str) -> String {
    let caracteres: Vec<char> = texto.chars().collect();
    let mut saida = String::with_capacity(texto.len() + 16);
    let mut posicao = 0;

    while posicao < caracteres.len() {
        let atual = caracteres[posicao];
        let comeca_numero = atual.is_ascii_digit()
            && (posicao == 0 || !caracteres[posicao - 1].is_alphanumeric());
        if !comeca_numero {
            saida.push(atual);
            posicao += 1;
            continue;
        }

        let mut fim_numero = posicao;
        while fim_numero < caracteres.len()
            && (caracteres[fim_numero].is_ascii_digit()
                || (matches!(caracteres[fim_numero], ',' | '.')
                    && caracteres.get(fim_numero + 1).is_some_and(char::is_ascii_digit)))
        {
            fim_numero += 1;
        }

        let mut fim_sufixo = fim_numero;
        while fim_sufixo < caracteres.len() && caracteres[fim_sufixo].is_alphabetic() {
            fim_sufixo += 1;
        }
        let colado_em_outra_palavra = caracteres.get(fim_sufixo).is_some_and(|c| c.is_alphanumeric());

        let numero: String = caracteres[posicao..fim_numero].iter().collect();
        let sufixo: String = caracteres[fim_numero..fim_sufixo].iter().collect();
        saida.push_str(&numero);

        match nome_da_unidade(&sufixo, numero == "1").filter(|_| !colado_em_outra_palavra) {
            Some(nome) => {
                saida.push(' ');
                saida.push_str(nome);
            }
            None => saida.push_str(&sufixo),
        }
        posicao = fim_sufixo;
    }
    saida
}

fn fechar_frase(texto: &str) -> String {
    let aparado = texto.trim();
    let precisa_de_ponto = aparado
        .chars()
        .last()
        .is_some_and(|ultimo| ultimo.is_alphanumeric() || matches!(ultimo, '%' | ')' | '"' | '”'));
    if precisa_de_ponto {
        format!("{aparado}.")
    } else {
        aparado.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titulo_sem_ponto_ganha_ponto_para_a_voz_parar() {
        assert_eq!(para_a_voz("Primeiro lugar geral"), "Primeiro lugar geral.");
    }

    #[test]
    fn frase_ja_pontuada_nao_muda() {
        assert_eq!(para_a_voz("Tudo certo."), "Tudo certo.");
        assert_eq!(para_a_voz("Tudo certo?"), "Tudo certo?");
        assert_eq!(para_a_voz("Lista:"), "Lista:");
    }

    #[test]
    fn segundos_viram_palavra() {
        assert_eq!(para_a_voz("Levou 2,0s para abrir."), "Levou 2,0 segundos para abrir.");
        assert_eq!(para_a_voz("Esperou 1s."), "Esperou 1 segundo.");
        assert_eq!(para_a_voz("Caiu para 300ms."), "Caiu para 300 milissegundos.");
        assert_eq!(para_a_voz("Durou 5min e 2h."), "Durou 5 minutos e 2 horas.");
    }

    #[test]
    fn sufixo_que_nao_e_unidade_fica_como_esta() {
        assert_eq!(para_a_voz("Foram 3 horas e 10kg."), "Foram 3 horas e 10kg.");
        assert_eq!(para_a_voz("Às 10h30 chegou."), "Às 10h30 chegou.");
        assert_eq!(para_a_voz("O modelo Q8s rodou."), "O modelo Q8s rodou.");
        assert_eq!(para_a_voz("Versão 2.0 saiu."), "Versão 2.0 saiu.");
    }

    #[test]
    fn muleta_repetida_vira_uma_so() {
        assert_eq!(para_a_voz("Foi assim, né, né, né, que começou."), "Foi assim, né, que começou.");
        assert_eq!(para_a_voz("Ficou bom, né."), "Ficou bom, né.");
    }
}
